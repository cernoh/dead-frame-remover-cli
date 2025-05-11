use image;
use once_cell::sync::Lazy;
use rayon::prelude::*;
use std::env;
use std::fs;
use std::fs::File;
use std::io::Cursor;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::Mutex;
use tempfile::tempdir;

const FFMPEG_EXECUTABLE: &[u8] = if cfg!(target_os = "windows") {
    include_bytes!("resources/ffmpeg-windows.zst")
} else if cfg!(target_os = "macos") {
    include_bytes!("resources/ffmpeg-mac.zst")
} else if cfg!(target_os = "linux") {
    include_bytes!("resources/ffmpeg-linux.zst")
} else {
    include_bytes!("resources/ffmpeg-linux.zst")
};

static FFMPEG_PATH: Lazy<Mutex<Option<String>>> = Lazy::new(|| Mutex::new(None));
static HW_ACCEL: Lazy<Mutex<Option<HWAccel>>> = Lazy::new(|| Mutex::new(None));

#[derive(Clone, Debug)]
struct HWAccel {
    name: String,
    decoder: Vec<String>,
    encoder: Vec<String>,
}

fn detect_hw_acceleration() -> Option<HWAccel> {
    let ffmpeg = get_ffmpeg_path();

    // Get available hardware accelerators
    let hwaccels = Command::new(&ffmpeg).args(["-hwaccels"]).output().ok()?;
    let hwaccels = String::from_utf8_lossy(&hwaccels.stdout);

    // Get available encoders
    let encoders = Command::new(&ffmpeg).args(["-encoders"]).output().ok()?;
    let encoders = String::from_utf8_lossy(&encoders.stdout);

    // Check for different hardware acceleration options
    let hw_configs: Vec<(&str, Vec<&str>, Vec<&str>, fn(&str, &str) -> bool)> = vec![
        // NVIDIA CUDA/NVENC
        (
            "cuda",
            vec!["cuda"],
            vec!["h264_nvenc", "-preset", "p4", "-tune", "hq"],
            |hw: &str, enc: &str| hw.contains("cuda") && enc.contains("h264_nvenc"),
        ),
        // Apple VideoToolbox
        (
            "videotoolbox",
            vec!["videotoolbox"],
            vec!["h264_videotoolbox"],
            |hw: &str, enc: &str| hw.contains("videotoolbox") && enc.contains("h264_videotoolbox"),
        ),
        // Intel QSV
        (
            "qsv",
            vec!["qsv"],
            vec!["h264_qsv"],
            |hw: &str, enc: &str| hw.contains("qsv") && enc.contains("h264_qsv"),
        ),
        // VA-API
        (
            "vaapi",
            vec!["vaapi", "-vaapi_device", "/dev/dri/renderD128"],
            vec![
                "-vaapi_device",
                "/dev/dri/renderD128",
                "-vf",
                "format=nv12,hwupload",
                "-c:v",
                "h264_vaapi",
            ],
            |hw: &str, enc: &str| hw.contains("vaapi") && enc.contains("h264_vaapi"),
        ),
    ];

    // Find the first supported hardware acceleration
    for (name, decoder, encoder, validator) in hw_configs {
        if validator(&hwaccels, &encoders) {
            return Some(HWAccel {
                name: name.to_string(),
                decoder: decoder.into_iter().map(String::from).collect(),
                encoder: encoder.into_iter().map(String::from).collect(),
            });
        }
    }

    None
}

fn get_hw_accel() -> Option<HWAccel> {
    let mut cached = HW_ACCEL.lock().unwrap();
    if cached.is_none() {
        *cached = detect_hw_acceleration();
    }
    cached.clone()
}

//TODO: add gpu prcoesssing to ffmpeg
fn extract_ffmpeg() -> std::io::Result<String> {
    use zstd::stream::read::Decoder;
    let temp_dir = env::temp_dir();
    let ffmpeg_path = temp_dir.join("ffmpeg");

    let compressed = Cursor::new(FFMPEG_EXECUTABLE);
    let mut decoder = Decoder::new(compressed)?;
    let mut out = File::create(&ffmpeg_path)?;
    std::io::copy(&mut decoder, &mut out)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = out.metadata()?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&ffmpeg_path, perms)?;
    }

    Ok(ffmpeg_path.to_string_lossy().into_owned())
}

fn get_ffmpeg_path() -> String {
    let mut cached = FFMPEG_PATH.lock().unwrap();
    if cached.is_none() {
        match extract_ffmpeg() {
            Ok(path) => *cached = Some(path),
            Err(e) => {
                eprintln!("Failed to extract ffmpeg!  :{}", e);
                std::process::exit(1);
            }
        }
    }
    cached.clone().unwrap()
}

fn collect_files(path: &Path) -> Vec<PathBuf> {
    if !path.exists() {
        return Vec::new();
    }

    if path.is_file() {
        if let Some(ext) = path.extension() {
            if ext == "png" {
                return vec![path.to_path_buf()];
            }
        }
        return Vec::new();
    }

    match fs::read_dir(path) {
        Ok(entries) => {
            entries
                .par_bridge() // Convert to parallel iterator
                .flat_map(|entry| {
                    if let Ok(entry) = entry {
                        let path = entry.path();
                        if path.is_dir() {
                            collect_files(&path)
                        } else if path.is_file()
                            && path.extension().map_or(false, |ext| ext == "png")
                        {
                            vec![path]
                        } else {
                            Vec::new()
                        }
                    } else {
                        Vec::new()
                    }
                })
                .collect()
        }
        Err(_) => Vec::new(),
    }
}

fn stitch_frames_into_video(folder: &str, output_file: &str) {
    let ffmpeg_path = get_ffmpeg_path();
    let input_pattern = format!("{}/frame_%04d.png", folder);

    let mut command_args = vec![
        "-framerate".to_string(),
        "30".to_string(),
        "-i".to_string(),
        input_pattern,
    ];

    // Add hardware encoding arguments if available
    if let Some(hw) = get_hw_accel() {
        command_args.extend(hw.encoder.iter().cloned());
    } else {
        // Fallback to CPU encoding
        command_args.extend(vec![
            "-c:v".to_string(),
            "libx264".to_string(),
            "-preset".to_string(),
            "fast".to_string(),
        ]);
    }

    command_args.extend(vec![
        "-threads".to_string(),
        "0".to_string(),
        "-pix_fmt".to_string(),
        "yuv420p".to_string(),
        output_file.to_string(),
    ]);

    let status = Command::new(ffmpeg_path)
        .args(command_args)
        .status()
        .expect("Failed to stitch frames into video");

    if !status.success() {
        eprintln!("FFmpeg failed to stitch video");
    }
}

fn generate_frames(input_file: &str) -> (String, tempfile::TempDir) {
    let temp_dir = tempdir().expect("Failed to create temp directory");
    let output_pattern = temp_dir.path().join("frame_%04d.png");
    let ffmpeg_path = get_ffmpeg_path();
    let output_pattern_str = output_pattern.to_str().unwrap();

    let mut command_args = Vec::new();

    // Add hardware decoding arguments if available
    if let Some(hw) = get_hw_accel() {
        command_args.extend(hw.decoder.iter().cloned());
    }

    command_args.extend(vec![
        "-i".to_string(),
        input_file.to_string(),
        "-threads".to_string(),
        "0".to_string(),
        output_pattern_str.to_string(),
    ]);

    Command::new(ffmpeg_path)
        .args(command_args)
        .output()
        .expect("Failed to execute ffmpeg");

    (
        output_pattern
            .parent()
            .unwrap()
            .to_str()
            .unwrap()
            .to_string(),
        temp_dir,
    )
}

fn compare_images_ssim_ffmpeg(image1: &str, image2: &str) -> f32 {
    let output = Command::new(get_ffmpeg_path())
        .arg("-i")
        .arg(image1)
        .arg("-i")
        .arg(image2)
        .arg("-filter_complex")
        .arg("ssim")
        .arg("-f")
        .arg("null")
        .stderr(Stdio::piped())
        .output()
        .expect("Failed to execute FFmpeg");

    let result = String::from_utf8_lossy(&output.stderr);

    // Parse the SSIM score from FFmpeg output
    // SSIM output looks like: "SSIM: All: 0.978"
    if let Some(ssim_value) = result.split("All: ").nth(1) {
        let ssim_score: f32 = ssim_value
            .split_whitespace()
            .next()
            .unwrap_or("0")
            .parse()
            .unwrap_or(0.0);
        return ssim_score;
    }

    0.0
}

fn compare_images_ssim_crate(
    image1: &str,
    image2: &str,
) -> Result<f32, Box<dyn std::error::Error>> {
    let image1 = image::open(image1).map_err(|e| format!("Failed to open first image: {}", e))?;
    let image2 = image::open(image2).map_err(|e| format!("Failed to open second image: {}", e))?;

    let grey1 = image1.to_luma8();
    let grey2 = image2.to_luma8();

    if grey1.dimensions() != grey2.dimensions() {
        return Err("images are different dimensions".into());
    }

    let (width, height) = grey1.dimensions();

    let ssim_sum: f32 = (0..height)
        .into_par_iter()
        .map(|y| {
            let k1 = 0.01;
            let k2 = 0.03;
            let l = 255.0;
            let c1 = (k1 * l as f32).powi(2);
            let c2 = (k2 * l as f32).powi(2);

            let mut row_sum = 0.0;
            for x in 0..width {
                let p1 = grey1.get_pixel(x, y)[0] as f32;
                let p2 = grey2.get_pixel(x, y)[0] as f32;

                //means
                let mu1 = p1;
                let mu2 = p2;

                //variance and covariance
                let sigma1_sq = (p1 - mu1).powi(2);
                let sigma2_sq = (p2 - mu2).powi(2);
                let sigma12 = (p1 - mu1) * (p2 - mu2);

                //calculate ssim
                let num = (2.0 * mu1 * mu2 + c1) * (2.0 * sigma12 + c2);
                let den = (mu1.powi(2) + mu2.powi(2) + c1) * (sigma1_sq + sigma2_sq + c2);
                row_sum += num / den;
            }
            row_sum
        })
        .sum();

    let ssim = ssim_sum / ((width * height) as f32);
    Ok(ssim)
}

pub async fn process_video(input_file: &str, output_directory: &str) -> Result<String, String> {
    println!("Starting video processing...");
    println!("Generating frames from input video: {}", input_file);
    let (frames_folder, _temp_dir) = generate_frames(input_file);

    println!("Collecting frames from: {}", frames_folder);
    let frames_vec: Vec<PathBuf> = collect_files(Path::new(&frames_folder));
    println!("Found {} frames to process", frames_vec.len());

    // Define batch size for comparing frames
    let batch_size = 20; // Adjust this based on your system's capabilities
    println!("Processing frames in batches of {}", batch_size);
    let bad_frames = Arc::new(Mutex::new(Vec::with_capacity(frames_vec.len())));

    println!("Starting frame comparison...");
    // Process frames in batches
    frames_vec.par_chunks(batch_size).for_each(|chunk| {
        // Local vector to store results for this batch
        let mut local_results = Vec::with_capacity(chunk.len());

        // Compare each frame with the next one within this batch
        for i in 0..chunk.len().saturating_sub(1) {
            let image1 = &chunk[i];
            let image2 = &chunk[i + 1];
            let score =
                compare_images_ssim_crate(&image1.to_string_lossy(), &image2.to_string_lossy())
                    .unwrap_or(0.0);
            local_results.push(score > 0.95);
        }

        // Last frame in batch can't be compared within batch
        if !chunk.is_empty() && chunk.len() < batch_size {
            local_results.push(false);
        }

        // Add local results to overall results
        let mut bad_frames_guard = bad_frames.lock().unwrap();
        bad_frames_guard.extend(local_results);
    });

    let mut bad_frames = bad_frames.lock().unwrap();
    // Ensure we have a result for each frame (except the last one)
    if frames_vec.is_empty() {
        return Err("No frames to process".to_string());
    }
    while bad_frames.len() < frames_vec.len() {
        bad_frames.push(false);
    }
    // Add false for the last frame
    bad_frames.push(false);

    println!("Removing duplicate frames...");
    // Remove bad frames
    let mut removed_count = 0;
    for (index, value) in frames_vec.iter().enumerate() {
        if bad_frames[index] {
            if let Err(e) = fs::remove_file(value) {
                eprintln!("Failed to remove file {}: {}", value.display(), e);
            } else {
                removed_count += 1;
            }
        }
    }

    println!("Removed {} duplicate frames", removed_count);

    println!("Stitching frames back into video...");
    let output_video = format!(
        "{}/{}_processed.mp4",
        output_directory,
        Path::new(input_file).file_stem().unwrap().to_str().unwrap()
    );
    stitch_frames_into_video(&frames_folder, &output_video);

    println!("Video processing complete!");
    Ok(output_video)
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 3 {
        eprintln!("Usage: {} <video_file> <output_directory>", args[0]);
        std::process::exit(1);
    }

    let input_file = &args[1];
    let output_dir = &args[2];

    let output_vid: Result<String, String> = process_video(input_file, output_dir).await;

    match output_vid {
        Ok(path) => println!("Video created: {}", path),
        Err(e) => eprintln!("error creating video: {}", e),
    }
}
