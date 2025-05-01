use std::env;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::process::Stdio;

fn collect_files(path: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();

    if let Ok(entries) = fs::read_dir(path) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                if let Some(ext) = path.extension() {
                    if ext == "png" {
                        files.push(path);
                    }
                }
            } else if path.is_dir() {
                files.extend(collect_files(&path));
            }
        }
    }

    files
}

fn generate_hash(input_file: &str) -> String {
    let output = Command::new(ffmpeg_executable)
        .args(&["-i", "input.mp4", "-f", "hash", "-hash", "sha256", "-"])
        .output()
        .expect("Failed to run ffmpeg");

    if output.status.success() {
        String::from_utf8_lossy(&output.stdout).to_string()
    } else {
        eprintln!("Error: {}", String::from_utf8_lossy(&output.stderr));
        "ERROR".to_string()
    }
}

fn generate_frames(input_file: &str) -> String {
    let input_path = Path::new(input_file);
    let stem = input_path
        .file_stem()
        .unwrap()
        .to_str()
        .unwrap()
        .replace(' ', "-");

    let output_folder = Path::new(&stem);

    if let Err(e) = fs::create_dir_all(&output_folder) {
        eprintln!("Failed to create output directory: {}", e);
        std::process::exit(1);
    }

    let output_pattern = format!("{}/frame_%04d.png", stem);

    // Determine the ffmpeg executable based on the OS
    let ffmpeg_executable = if cfg!(target_os = "windows") {
        "resources/ffmpeg.exe"
    } else {
        "ffmpeg"
    };

    Command::new(ffmpeg_executable)
        .args(&["-i", input_file, &output_pattern])
        .output()
        .expect("Failed to execute ffmpeg");

    stem
}

fn compare_images_ssim(image1: &str, image2: &str) -> f32 {
    // Determine the ffmpeg executable based on the OS
    let ffmpeg_executable = if cfg!(target_os = "windows") {
        "resources/ffmpeg.exe"
    } else {
        "ffmpeg"
    };

    let output = Command::new(ffmpeg_executable)
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

fn main() {
    // Get command-line arguments
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: {} <video_file>", args[0]);
        std::process::exit(1);
    }

    let input_file = &args[1];

    let frames_folder: &str = &generate_frames(input_file);

    let frames_vec: Vec<PathBuf> = collect_files(Path::new(frames_folder));

    for (index, value) in frames_vec.iter().enumerate() {
        if index == 0 {
            continue;
        }

        let ssim_val = compare_images_ssim(
            value.to_str().unwrap(),
            frames_vec[index - 1].to_str().unwrap(),
        );
        if ssim_val > 0.98 {}
    }
}
