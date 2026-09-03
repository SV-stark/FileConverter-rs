use crate::error::{FileConverterError, Result};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Read;
use std::path::PathBuf;

/// SHA-256 of `ffmpeg-7.0.2-essentials_build.zip` from
/// https://github.com/GyanD/codexffmpeg/releases/download/7.0.2/ffmpeg-7.0.2-essentials_build.zip
/// (verified 2026-08-20 against the official GitHub release artifact).
const FFMPEG_SHA256: &str = "D5308D30872B2739CF53169DF61FABA8639D39A19B20B91E611C177EF676F64C";

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
        "https://www.gyan.dev/ffmpeg/builds/packages/ffmpeg-7.0.2-essentials_build.zip",
    ];

    let temp_zip_file = tempfile::Builder::new()
        .prefix("fc_ffmpeg_")
        .suffix(".zip")
        .tempfile_in(parent)
        .or_else(|_| {
            tempfile::Builder::new()
                .prefix("fc_ffmpeg_")
                .suffix(".zip")
                .tempfile()
        })
        .map_err(FileConverterError::Io)?;
    let temp_zip_path = temp_zip_file.path().to_path_buf();
    let mut downloaded = false;

    let config = ureq::config::Config::builder()
        .timeout_global(Some(std::time::Duration::from_secs(90)))
        .build();
    let agent: ureq::Agent = config.into();

    for url in &download_sources {
        if let Ok(response) = agent.get(*url).call()
            && let Ok(mut out) = fs::File::create(&temp_zip_path)
        {
            let mut reader = response.into_body().into_reader();
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

    // Verify package integrity against the pinned SHA-256 before extraction.
    let mut file_for_hash = fs::File::open(&temp_zip_path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = file_for_hash.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let digest = hex::encode(hasher.finalize());
    if !digest.eq_ignore_ascii_case(FFMPEG_SHA256) {
        let _ = fs::remove_file(&temp_zip_path);
        return Err(FileConverterError::Ffmpeg(format!(
            "Downloaded FFmpeg package failed SHA-256 integrity verification (expected {}, got {})",
            FFMPEG_SHA256, digest
        )));
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
