use crate::error::{FileConverterError, Result};
use std::env;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

pub fn get_ffmpeg_binary_path() -> PathBuf {
    // 1. Check local directory / system PATH
    if let Ok(path) = which::which("ffmpeg.exe") {
        return path;
    }
    if let Ok(mut exe_dir) = env::current_exe() {
        exe_dir.pop();
        let candidate = exe_dir.join("ffmpeg.exe");
        if candidate.exists() {
            return candidate;
        }
    }

    // 2. Check local app data bin folder
    let local_app_data = env::var("LOCALAPPDATA").unwrap_or_default();
    Path::new(&local_app_data)
        .join("FileConverter")
        .join("bin")
        .join("ffmpeg.exe")
}

pub fn ensure_ffmpeg_available() -> Result<PathBuf> {
    let target_path = get_ffmpeg_binary_path();
    if target_path.exists() {
        return Ok(target_path);
    }

    let parent = target_path
        .parent()
        .ok_or_else(|| FileConverterError::Invalid("Invalid FFmpeg target path".to_string()))?;
    fs::create_dir_all(parent)?;

    // Download URL for static ffmpeg zip release
    let download_url = "https://github.com/GyanD/codexffmpeg/releases/download/7.0.2/ffmpeg-7.0.2-essentials_build.zip";

    let response = ureq::get(download_url)
        .call()
        .map_err(|e| FileConverterError::Ffmpeg(format!("Failed to download FFmpeg: {:?}", e)))?;

    let mut zip_bytes = Vec::new();
    response
        .into_reader()
        .read_to_end(&mut zip_bytes)
        .map_err(FileConverterError::Io)?;

    let cursor = std::io::Cursor::new(zip_bytes);
    let mut archive = zip::ZipArchive::new(cursor)
        .map_err(|e| FileConverterError::Invalid(format!("Failed to parse FFmpeg zip: {:?}", e)))?;

    for i in 0..archive.len() {
        let mut file = archive
            .by_index(i)
            .map_err(|e| FileConverterError::Invalid(e.to_string()))?;

        if file.name().ends_with("ffmpeg.exe") {
            let mut out_file = fs::File::create(&target_path)?;
            std::io::copy(&mut file, &mut out_file)?;
            return Ok(target_path);
        }
    }

    Err(FileConverterError::Ffmpeg(
        "ffmpeg.exe not found in downloaded release package".to_string(),
    ))
}
