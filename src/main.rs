use std::env;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::process::Stdio;

use tempfile::tempdir;

// Determine the ffmpeg executable based on the OS
const FFMPEG_EXECUTABLE: &str = if cfg!(target_os = "windows") {
    "resources/ffmpeg.exe"
} else {
    "resources/ffmpeg"
};

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

fn stitch_frames_into_video(folder: &str, output_file: &str) {
    let input_pattern = format!("{}/frame_%04d.png", folder);

    let status = Command::new(FFMPEG_EXECUTABLE)
        .args(&[
            "-framerate",
            "30",
            "-i",
            &input_pattern,
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            output_file,
        ])
        .status()
        .expect("Failed to stitch frames into video");

    if !status.success() {
        eprintln!("FFmpeg failed to stitch video");
    }
}

fn generate_hash(input_file: &str) -> String {
    let output = Command::new(FFMPEG_EXECUTABLE)
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

fn generate_frames(input_file: &str) -> (String, tempfile::TempDir) {
    let temp_dir = tempdir().expect("Failed to create temp directory");
    let output_pattern = temp_dir.path().join("frame_%04d.png");

    let output_pattern_str = output_pattern.to_str().unwrap();

    Command::new(FFMPEG_EXECUTABLE)
        .args(&["-i", input_file, output_pattern_str])
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

fn compare_images_ssim(image1: &str, image2: &str) -> f32 {
    let output = Command::new(FFMPEG_EXECUTABLE)
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
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: {} <video_file>", args[0]);
        std::process::exit(1);
    }

    let input_file = &args[1];

    let (frames_folder, _temp_dir) = generate_frames(input_file);

    let frames_vec: Vec<PathBuf> = collect_files(Path::new(&frames_folder));

    let mut bad_frames: Vec<bool> = Vec::new();

    for index in 0..frames_vec.len() - 1 {
        let ssim_val = compare_images_ssim(
            frames_vec[index].to_str().unwrap(),
            frames_vec[index + 1].to_str().unwrap(),
        );

        if ssim_val > 0.98 {
            bad_frames.push(true);
        } else {
            bad_frames.push(false);
        }
    }

    bad_frames.push(false);

    for (index, value) in frames_vec.iter().enumerate() {
        if bad_frames[index] {
            match fs::remove_file(value) {
                Ok(_) => println!("removed dead frame"),
                Err(e) => eprintln!("failed to delete file {}", e),
            }
        }
    }

    let output_video = format!(
        "{}_processed.mp4",
        Path::new(input_file).file_stem().unwrap().to_str().unwrap()
    );
    stitch_frames_into_video(&frames_folder, &output_video);
    println!("Video created: {}", output_video);
}
