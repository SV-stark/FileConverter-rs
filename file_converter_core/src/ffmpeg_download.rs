use crate::error::{FileConverterError, Result};
use std::fs;
use std::path::PathBuf;

pub fn get_ffmpeg_binary_path() -> PathBuf {
    crate::ffmpeg::get_ffmpeg_path()
}

pub fn ensure_ffmpeg_available() -> Result<PathBuf> {
    let target_path = get_ffmpeg_binary_path();
    if target_path.exists() {
        return Ok(target_path);
    }

    let local_app_data = std::env::var("LOCALAPPDATA").unwrap_or_default();
    let default_bin = std::path::Path::new(&local_app_data)
        .join("FileConverter")
        .join("bin")
        .join("ffmpeg.exe");

    let final_target = if target_path.file_name() == Some(std::ffi::OsStr::new("ffmpeg.exe"))
        && target_path.parent().is_some()
    {
        target_path
    } else {
        default_bin
    };

    let parent = final_target
        .parent()
        .ok_or_else(|| FileConverterError::Invalid("Invalid FFmpeg target path".to_string()))?;
    fs::create_dir_all(parent)?;

    // Download URLs with fallback mirrors
    let download_sources = [
        "https://github.com/GyanD/codexffmpeg/releases/download/7.0.2/ffmpeg-7.0.2-essentials_build.zip",
        "https://www.gyan.dev/ffmpeg/builds/ffmpeg-release-essentials.zip",
    ];

    let temp_zip_path = parent.join("ffmpeg_temp.zip");
    let mut downloaded = false;

    for url in &download_sources {
        if let Ok(response) = ureq::get(url)
            .timeout(std::time::Duration::from_secs(90))
            .call()
            && let Ok(mut out) = fs::File::create(&temp_zip_path)
        {
            let mut reader = response.into_reader();
            if std::io::copy(&mut reader, &mut out).is_ok() {
                downloaded = true;
                break;
            }
        }
    }

    if !downloaded {
        return Err(FileConverterError::Ffmpeg(
            "Failed to download FFmpeg release package from all available mirrors".to_string(),
        ));
    }

    let zip_file = fs::File::open(&temp_zip_path)?;
    let mut archive = zip::ZipArchive::new(zip_file)
        .map_err(|e| FileConverterError::Invalid(format!("Failed to parse FFmpeg zip: {:?}", e)))?;

    let mut found = false;
    for i in 0..archive.len() {
        let mut file = archive
            .by_index(i)
            .map_err(|e| FileConverterError::Invalid(e.to_string()))?;

        if file.name().ends_with("ffmpeg.exe") {
            let mut out_file = fs::File::create(&final_target)?;
            std::io::copy(&mut file, &mut out_file)?;
            found = true;
            break;
        }
    }

    let _ = fs::remove_file(&temp_zip_path);

    if found {
        // Validate executable integrity (must be non-empty and start with MZ header)
        if let Ok(header) = fs::read(&final_target)
            && header.len() > 1024
            && header.starts_with(b"MZ")
        {
            return Ok(final_target);
        }
        let _ = fs::remove_file(&final_target);
        Err(FileConverterError::Ffmpeg(
            "Downloaded FFmpeg binary failed executable integrity verification".to_string(),
        ))
    } else {
        Err(FileConverterError::Ffmpeg(
            "ffmpeg.exe not found in downloaded release package".to_string(),
        ))
    }
}
