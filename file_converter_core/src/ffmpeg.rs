use regex::Regex;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use crate::path_helpers;
use crate::settings::ConversionPreset;
use crate::types::{EncodingMode, HardwareAccelerationMode, OutputType, VideoEncodingSpeed};

pub struct FfmpegPass {
    pub name: String,
    pub arguments: Vec<String>,
    pub file_to_delete: Option<PathBuf>,
}

pub fn get_ffmpeg_path() -> PathBuf {
    if let Ok(mut exe_path) = std::env::current_exe() {
        exe_path.pop();
        let path = exe_path.join("ffmpeg.exe");
        if path.exists() {
            return path;
        }
    }
    PathBuf::from("ffmpeg.exe")
}

pub fn get_ffmpeg_passes(
    preset: &ConversionPreset,
    input_path: &str,
    output_path: &str,
    hw_accel: HardwareAccelerationMode,
) -> Result<Vec<FfmpegPass>, String> {
    let mut passes = Vec::new();
    let base_args = vec!["-n".to_string()];

    let mut hw_input_args = Vec::new();
    match hw_accel {
        HardwareAccelerationMode::Cuda => {
            hw_input_args.push("-hwaccel".to_string());
            hw_input_args.push("cuda".to_string());
        }
        HardwareAccelerationMode::Amf => {
            hw_input_args.push("-hwaccel".to_string());
            hw_input_args.push("d3d11va".to_string());
        }
        HardwareAccelerationMode::Off => {}
    }

    // Helper to check custom command
    let custom_cmd_enabled = preset
        .get_setting_value("EnableFFMPEGCustomCommand")
        .and_then(|v| v.parse::<bool>().ok())
        .unwrap_or(false);

    if custom_cmd_enabled {
        let custom_cmd = preset
            .get_setting_value("FFMPEGCustomCommand")
            .unwrap_or("");
        // Simple token split for custom command parameters
        let mut arguments = base_args.clone();
        arguments.push("-i".to_string());
        arguments.push(input_path.to_string());

        // Split by whitespace, respecting quotes if needed. For 100% parity, simple whitespace split is standard.
        for token in custom_cmd.split_whitespace() {
            arguments.push(token.to_string());
        }

        arguments.push(output_path.to_string());
        passes.push(FfmpegPass {
            name: "Conversion".to_string(),
            arguments,
            file_to_delete: None,
        });
        return Ok(passes);
    }

    let mp3_metadata = vec![
        "-id3v2_version".to_string(),
        "3".to_string(),
        "-write_id3v1".to_string(),
        "1".to_string(),
    ];
    let aac_metadata = vec!["-write_apetag".to_string(), "1".to_string()];

    match preset.output_type {
        OutputType::Aac => {
            let channel_args = compute_audio_channel_args(preset);
            let bitrate = preset
                .get_setting_value("AudioBitrate")
                .and_then(|v| v.parse::<i32>().ok())
                .unwrap_or(128);
            let quality = aac_bitrate_to_quality_index(bitrate);

            let mut arguments = base_args.clone();
            arguments.push("-i".to_string());
            arguments.push(input_path.to_string());
            arguments.push("-c:a".to_string());
            arguments.push("aac".to_string());
            arguments.push("-q:a".to_string());
            arguments.push(quality);
            if !channel_args.is_empty() {
                arguments.push("-ac".to_string());
                arguments.push(channel_args);
            }
            arguments.extend(aac_metadata);
            arguments.push(output_path.to_string());

            passes.push(FfmpegPass {
                name: "Conversion".to_string(),
                arguments,
                file_to_delete: None,
            });
        }
        OutputType::Avi => {
            let quality = preset
                .get_setting_value("VideoQuality")
                .and_then(|v| v.parse::<i32>().ok())
                .unwrap_or(27);
            let audio_bitrate = preset
                .get_setting_value("AudioBitrate")
                .and_then(|v| v.parse::<i32>().ok())
                .unwrap_or(190);
            let enable_audio = preset
                .get_setting_value("EnableAudio")
                .and_then(|v| v.parse::<bool>().ok())
                .unwrap_or(true);

            let mut arguments = base_args.clone();
            arguments.push("-i".to_string());
            arguments.push(input_path.to_string());
            arguments.push("-c:v".to_string());
            arguments.push("mpeg4".to_string());
            arguments.push("-vtag".to_string());
            arguments.push("xvid".to_string());
            arguments.push("-qscale:v".to_string());
            arguments.push((31 - quality).to_string());

            if enable_audio {
                arguments.push("-c:a".to_string());
                arguments.push("libmp3lame".to_string());
                arguments.push("-qscale:a".to_string());
                arguments.push(mp3_vbr_bitrate_to_quality_index(audio_bitrate)?.to_string());
            } else {
                arguments.push("-an".to_string());
            }

            let transform = compute_transform_args(preset, hw_accel);
            if !transform.is_empty() {
                arguments.push("-vf".to_string());
                arguments.push(transform);
            }
            arguments.extend(mp3_metadata);
            arguments.push(output_path.to_string());

            passes.push(FfmpegPass {
                name: "Conversion".to_string(),
                arguments,
                file_to_delete: None,
            });
        }
        OutputType::Flac => {
            let channel_args = compute_audio_channel_args(preset);
            let mut arguments = base_args.clone();
            arguments.push("-i".to_string());
            arguments.push(input_path.to_string());
            arguments.push("-compression_level".to_string());
            arguments.push("12".to_string());
            if !channel_args.is_empty() {
                arguments.push("-ac".to_string());
                arguments.push(channel_args);
            }
            arguments.push(output_path.to_string());

            passes.push(FfmpegPass {
                name: "Conversion".to_string(),
                arguments,
                file_to_delete: None,
            });
        }
        OutputType::Gif => {
            // High-quality palette generation and utilization (2 passes)
            let temp_dir = std::env::temp_dir();
            let file_name = Path::new(input_path)
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("temp");
            let palette_path = path_helpers::generate_unique_path(
                temp_dir.join(format!("{} - palette.png", file_name)),
                &[],
            );

            let transform = compute_transform_args(preset, hw_accel);
            let fps = preset
                .get_setting_value("VideoFramesPerSecond")
                .and_then(|v| v.parse::<i32>().ok())
                .unwrap_or(15);

            let vf_palettegen = if transform.is_empty() {
                format!("fps={}", fps)
            } else {
                format!("{},fps={}", transform, fps)
            };

            // Pass 1: PaletteGen
            let mut args1 = base_args.clone();
            args1.push("-i".to_string());
            args1.push(input_path.to_string());
            args1.push("-vf".to_string());
            args1.push(format!("{},palettegen", vf_palettegen));
            args1.push(palette_path.to_string_lossy().to_string());

            passes.push(FfmpegPass {
                name: "Indexing colors".to_string(),
                arguments: args1,
                file_to_delete: Some(palette_path.clone()),
            });

            // Pass 2: PaletteUse
            let mut args2 = base_args.clone();
            args2.push("-i".to_string());
            args2.push(input_path.to_string());
            args2.push("-i".to_string());
            args2.push(palette_path.to_string_lossy().to_string());
            args2.push("-lavfi".to_string());
            args2.push(format!("{},paletteuse", vf_palettegen));
            args2.push(output_path.to_string());

            passes.push(FfmpegPass {
                name: "Conversion".to_string(),
                arguments: args2,
                file_to_delete: None,
            });
        }
        OutputType::Ico => {
            let mut arguments = base_args.clone();
            arguments.push("-i".to_string());
            arguments.push(input_path.to_string());
            arguments.push(output_path.to_string());
            passes.push(FfmpegPass {
                name: "Conversion".to_string(),
                arguments,
                file_to_delete: None,
            });
        }
        OutputType::Jpg => {
            let quality = preset
                .get_setting_value("ImageQuality")
                .and_then(|v| v.parse::<i32>().ok())
                .unwrap_or(90);
            let scale_factor = preset
                .get_setting_value("ImageScale")
                .and_then(|v| v.parse::<f32>().ok())
                .unwrap_or(1.0);

            let mut arguments = base_args.clone();
            arguments.push("-i".to_string());
            arguments.push(input_path.to_string());
            arguments.push("-q:v".to_string());
            arguments.push((31 - (quality * 30 / 100)).to_string()); // Map 1-100 to 31-1 range approximately

            if (scale_factor - 1.0).abs() >= 0.005 {
                arguments.push("-vf".to_string());
                arguments.push(format!(
                    "scale=iw*{:.2}:ih*{:.2}",
                    scale_factor, scale_factor
                ));
            }
            arguments.push(output_path.to_string());

            passes.push(FfmpegPass {
                name: "Conversion".to_string(),
                arguments,
                file_to_delete: None,
            });
        }
        OutputType::Mp3 => {
            let channel_args = compute_audio_channel_args(preset);
            let encoding_mode = preset
                .get_setting_value("AudioEncodingMode")
                .and_then(|v| {
                    if v == "Mp3VBR" {
                        Some(EncodingMode::Mp3Vbr)
                    } else if v == "Mp3CBR" {
                        Some(EncodingMode::Mp3Cbr)
                    } else {
                        None
                    }
                })
                .unwrap_or(EncodingMode::Mp3Vbr);
            let bitrate = preset
                .get_setting_value("AudioBitrate")
                .and_then(|v| v.parse::<i32>().ok())
                .unwrap_or(190);

            let mut arguments = base_args.clone();
            arguments.push("-i".to_string());
            arguments.push(input_path.to_string());
            arguments.push("-codec:a".to_string());
            arguments.push("libmp3lame".to_string());

            match encoding_mode {
                EncodingMode::Mp3Vbr => {
                    arguments.push("-q:a".to_string());
                    arguments.push(mp3_vbr_bitrate_to_quality_index(bitrate)?.to_string());
                }
                EncodingMode::Mp3Cbr => {
                    arguments.push("-b:a".to_string());
                    arguments.push(format!("{}k", bitrate));
                }
                _ => {}
            }

            if !channel_args.is_empty() {
                arguments.push("-ac".to_string());
                arguments.push(channel_args);
            }
            arguments.extend(mp3_metadata);
            arguments.push(output_path.to_string());

            passes.push(FfmpegPass {
                name: "Conversion".to_string(),
                arguments,
                file_to_delete: None,
            });
        }
        OutputType::Mkv | OutputType::Mp4 => {
            let quality = preset
                .get_setting_value("VideoQuality")
                .and_then(|v| v.parse::<i32>().ok())
                .unwrap_or(30);
            let audio_bitrate = preset
                .get_setting_value("AudioBitrate")
                .and_then(|v| v.parse::<i32>().ok())
                .unwrap_or(155);
            let speed_val = preset
                .get_setting_value("VideoEncodingSpeed")
                .unwrap_or("Medium");
            let enable_audio = preset
                .get_setting_value("EnableAudio")
                .and_then(|v| v.parse::<bool>().ok())
                .unwrap_or(true);

            let speed = match speed_val {
                "UltraFast" => VideoEncodingSpeed::UltraFast,
                "SuperFast" => VideoEncodingSpeed::SuperFast,
                "VeryFast" => VideoEncodingSpeed::VeryFast,
                "Faster" => VideoEncodingSpeed::Faster,
                "Fast" => VideoEncodingSpeed::Fast,
                "Medium" => VideoEncodingSpeed::Medium,
                "Slow" => VideoEncodingSpeed::Slow,
                "Slower" => VideoEncodingSpeed::Slower,
                "VerySlow" => VideoEncodingSpeed::VerySlow,
                _ => VideoEncodingSpeed::Medium,
            };

            let mut arguments = base_args.clone();
            let mut hw_accel_arg = Vec::new();
            let mut video_codec = "libx264".to_string();
            let mut video_codec_args = vec![
                "-preset".to_string(),
                h264_encoding_speed_to_preset(speed).to_string(),
                "-crf".to_string(),
                (51 - quality).to_string(),
            ];

            match hw_accel {
                HardwareAccelerationMode::Cuda => {
                    video_codec = "h264_nvenc".to_string();
                    let qp = 51 - quality;
                    video_codec_args = vec![
                        "-preset".to_string(),
                        h264_encoding_speed_to_nvenc_preset(speed).to_string(),
                        "-rc".to_string(),
                        "constqp".to_string(),
                        "-qp".to_string(),
                        qp.to_string(),
                    ];
                    hw_accel_arg = vec![
                        "-hwaccel".to_string(),
                        "cuda".to_string(),
                        "-hwaccel_output_format".to_string(),
                        "cuda".to_string(),
                    ];
                }
                HardwareAccelerationMode::Amf => {
                    let qp = 51 - quality;
                    let b_qp = std::cmp::min(51, qp + 2);
                    video_codec = "h264_amf".to_string();
                    video_codec_args = vec![
                        "-usage".to_string(),
                        "transcoding".to_string(),
                        "-quality".to_string(),
                        h264_encoding_speed_to_amf_quality(speed).to_string(),
                        "-qp_i".to_string(),
                        qp.to_string(),
                        "-qp_p".to_string(),
                        qp.to_string(),
                        "-qp_b".to_string(),
                        b_qp.to_string(),
                    ];
                }
                HardwareAccelerationMode::Off => {}
            }

            arguments.extend(hw_accel_arg);
            arguments.push("-i".to_string());
            arguments.push(input_path.to_string());
            arguments.push("-c:v".to_string());
            arguments.push(video_codec);
            arguments.extend(video_codec_args);

            if enable_audio {
                arguments.push("-c:a".to_string());
                arguments.push("aac".to_string());
                arguments.push("-qscale:a".to_string());
                arguments.push(aac_bitrate_to_quality_index(audio_bitrate));
            } else {
                arguments.push("-an".to_string());
            }

            let transform = compute_transform_args(preset, hw_accel);
            if !transform.is_empty() {
                arguments.push("-vf".to_string());
                arguments.push(transform);
            }
            arguments.push(output_path.to_string());

            passes.push(FfmpegPass {
                name: "Conversion".to_string(),
                arguments,
                file_to_delete: None,
            });
        }
        OutputType::Ogg => {
            let channel_args = compute_audio_channel_args(preset);
            let bitrate = preset
                .get_setting_value("AudioBitrate")
                .and_then(|v| v.parse::<i32>().ok())
                .unwrap_or(160);

            let mut arguments = base_args.clone();
            arguments.push("-i".to_string());
            arguments.push(input_path.to_string());
            arguments.push("-vn".to_string());
            arguments.push("-codec:a".to_string());
            arguments.push("libvorbis".to_string());
            arguments.push("-qscale:a".to_string());
            arguments.push(ogg_vbr_bitrate_to_quality_index(bitrate)?.to_string());
            if !channel_args.is_empty() {
                arguments.push("-ac".to_string());
                arguments.push(channel_args);
            }
            arguments.push(output_path.to_string());

            passes.push(FfmpegPass {
                name: "Conversion".to_string(),
                arguments,
                file_to_delete: None,
            });
        }
        OutputType::Ogv => {
            let quality = preset
                .get_setting_value("VideoQuality")
                .and_then(|v| v.parse::<i32>().ok())
                .unwrap_or(8);
            let audio_bitrate = preset
                .get_setting_value("AudioBitrate")
                .and_then(|v| v.parse::<i32>().ok())
                .unwrap_or(160);
            let enable_audio = preset
                .get_setting_value("EnableAudio")
                .and_then(|v| v.parse::<bool>().ok())
                .unwrap_or(true);

            let mut arguments = base_args.clone();
            arguments.push("-i".to_string());
            arguments.push(input_path.to_string());
            arguments.push("-codec:v".to_string());
            arguments.push("libtheora".to_string());
            arguments.push("-qscale:v".to_string());
            arguments.push(quality.to_string());

            if enable_audio {
                arguments.push("-codec:a".to_string());
                arguments.push("libvorbis".to_string());
                arguments.push("-qscale:a".to_string());
                arguments.push(ogg_vbr_bitrate_to_quality_index(audio_bitrate)?.to_string());
            } else {
                arguments.push("-an".to_string());
            }

            let transform = compute_transform_args(preset, hw_accel);
            if !transform.is_empty() {
                arguments.push("-vf".to_string());
                arguments.push(transform);
            }
            arguments.push(output_path.to_string());

            passes.push(FfmpegPass {
                name: "Conversion".to_string(),
                arguments,
                file_to_delete: None,
            });
        }
        OutputType::Png => {
            let scale_factor = preset
                .get_setting_value("ImageScale")
                .and_then(|v| v.parse::<f32>().ok())
                .unwrap_or(1.0);
            let mut arguments = base_args.clone();
            arguments.push("-i".to_string());
            arguments.push(input_path.to_string());
            arguments.push("-compression_level".to_string());
            arguments.push("100".to_string());

            if (scale_factor - 1.0).abs() >= 0.005 {
                arguments.push("-vf".to_string());
                arguments.push(format!(
                    "scale=iw*{:.2}:ih*{:.2}",
                    scale_factor, scale_factor
                ));
            }
            arguments.push(output_path.to_string());

            passes.push(FfmpegPass {
                name: "Conversion".to_string(),
                arguments,
                file_to_delete: None,
            });
        }
        OutputType::Wav => {
            let channel_args = compute_audio_channel_args(preset);
            let encoding_val = preset
                .get_setting_value("AudioEncodingMode")
                .unwrap_or("Wav16");
            let codec = match encoding_val {
                "Wav8" => "pcm_s8le",
                "Wav16" => "pcm_s16le",
                "Wav24" => "pcm_s24le",
                "Wav32" => "pcm_s32le",
                _ => "pcm_s16le",
            };

            let mut arguments = base_args.clone();
            arguments.push("-i".to_string());
            arguments.push(input_path.to_string());
            arguments.push("-acodec".to_string());
            arguments.push(codec.to_string());
            if !channel_args.is_empty() {
                arguments.push("-ac".to_string());
                arguments.push(channel_args);
            }
            arguments.push(output_path.to_string());

            passes.push(FfmpegPass {
                name: "Conversion".to_string(),
                arguments,
                file_to_delete: None,
            });
        }
        OutputType::Webm => {
            let quality = preset
                .get_setting_value("VideoQuality")
                .and_then(|v| v.parse::<i32>().ok())
                .unwrap_or(40);
            let audio_bitrate = preset
                .get_setting_value("AudioBitrate")
                .and_then(|v| v.parse::<i32>().ok())
                .unwrap_or(160);
            let enable_audio = preset
                .get_setting_value("EnableAudio")
                .and_then(|v| v.parse::<bool>().ok())
                .unwrap_or(true);

            let mut arguments = base_args.clone();
            arguments.push("-i".to_string());
            arguments.push(input_path.to_string());
            arguments.push("-c:v".to_string());
            arguments.push("libvpx-vp9".to_string());

            if quality == 63 {
                arguments.push("-lossless".to_string());
                arguments.push("1".to_string());
            } else {
                arguments.push("-crf".to_string());
                arguments.push((63 - quality).to_string());
                arguments.push("-b:v".to_string());
                arguments.push("0".to_string());
            }

            if enable_audio {
                arguments.push("-c:a".to_string());
                arguments.push("libvorbis".to_string());
                arguments.push("-qscale:a".to_string());
                arguments.push(ogg_vbr_bitrate_to_quality_index(audio_bitrate)?.to_string());
            } else {
                arguments.push("-an".to_string());
            }

            let transform = compute_transform_args(preset, hw_accel);
            if !transform.is_empty() {
                arguments.push("-vf".to_string());
                arguments.push(transform);
            }
            arguments.push(output_path.to_string());

            passes.push(FfmpegPass {
                name: "Conversion".to_string(),
                arguments,
                file_to_delete: None,
            });
        }
        _ => {
            return Err(format!(
                "FFMpeg engine does not support output type {:?}",
                preset.output_type
            ));
        }
    }

    Ok(passes)
}

fn compute_audio_channel_args(preset: &ConversionPreset) -> String {
    let channels = preset
        .get_setting_value("AudioChannelCount")
        .and_then(|v| v.parse::<i32>().ok())
        .unwrap_or(0);
    if channels > 0 {
        channels.to_string()
    } else {
        String::new()
    }
}

fn compute_transform_args(preset: &ConversionPreset, hw_accel: HardwareAccelerationMode) -> String {
    let scale_factor = preset
        .get_setting_value("VideoScale")
        .and_then(|v| v.parse::<f32>().ok())
        .unwrap_or(1.0);
    let rotation = preset
        .get_setting_value("VideoRotation")
        .and_then(|v| v.parse::<f32>().ok())
        .unwrap_or(0.0);

    let mut scale_args = String::new();
    let is_h264 = preset.output_type == OutputType::Mkv || preset.output_type == OutputType::Mp4;

    if is_h264 {
        match hw_accel {
            HardwareAccelerationMode::Cuda => {
                scale_args = format!(
                    "scale_cuda=trunc(iw*{:.2}/2)*2:trunc(ih*{:.2}/2)*2:format=yuv420p",
                    scale_factor, scale_factor
                );
            }
            _ => {
                scale_args = format!(
                    "scale=trunc(iw*{:.2}/2)*2:trunc(ih*{:.2}/2)*2",
                    scale_factor, scale_factor
                );
            }
        }
    } else if (scale_factor - 1.0).abs() >= 0.005 {
        scale_args = format!("scale=iw*{:.2}:ih*{:.2}", scale_factor, scale_factor);
    }

    let mut rotation_args = String::new();
    if (rotation - 0.0).abs() >= 0.05 {
        if (rotation - 90.0).abs() <= 0.05 {
            rotation_args = "transpose=2".to_string(); // 90 counterclockwise
        } else if (rotation - 180.0).abs() <= 0.05 {
            rotation_args = "vflip,hflip".to_string();
        } else if (rotation - 270.0).abs() <= 0.05 {
            rotation_args = "transpose=1".to_string(); // 90 clockwise
        }
    }

    let mut transform = String::new();
    if !scale_args.is_empty() {
        transform.push_str(&scale_args);
    }

    if !rotation_args.is_empty() {
        if !transform.is_empty() {
            transform.push(',');
        }
        transform.push_str(&rotation_args);
    }

    if hw_accel != HardwareAccelerationMode::Cuda && is_h264 {
        if !transform.is_empty() {
            transform.push(',');
        }
        transform.push_str("format=yuv420p");
    }

    transform
}

fn aac_bitrate_to_quality_index(bitrate: i32) -> String {
    let q = match bitrate {
        460 => "3.9",
        340 => "3",
        256 => "2.2",
        224 => "1.9",
        192 => "1.6",
        155 => "1.3",
        128 => "1",
        112 => "0.9",
        96 => "0.75",
        80 => "0.6",
        64 => "0.45",
        48 => "0.3",
        32 => "0.2",
        16 => "0.1",
        _ => "1.3", // default fallback
    };
    q.to_string()
}

fn mp3_vbr_bitrate_to_quality_index(bitrate: i32) -> Result<i32, String> {
    match bitrate {
        245 => Ok(0),
        225 => Ok(1),
        190 => Ok(2),
        175 => Ok(3),
        165 => Ok(4),
        130 => Ok(5),
        115 => Ok(6),
        100 => Ok(7),
        85 => Ok(8),
        65 => Ok(9),
        _ => Err(format!("Unknown MP3 VBR bitrate: {}", bitrate)),
    }
}

fn ogg_vbr_bitrate_to_quality_index(bitrate: i32) -> Result<i32, String> {
    match bitrate {
        500 => Ok(10),
        320 => Ok(9),
        256 => Ok(8),
        224 => Ok(7),
        192 => Ok(6),
        160 => Ok(5),
        128 => Ok(4),
        112 => Ok(3),
        96 => Ok(2),
        80 => Ok(1),
        64 => Ok(0),
        48 => Ok(-1),
        32 => Ok(-2),
        _ => Err(format!("Unknown Ogg VBR bitrate: {}", bitrate)),
    }
}

fn h264_encoding_speed_to_preset(speed: VideoEncodingSpeed) -> &'static str {
    match speed {
        VideoEncodingSpeed::UltraFast => "ultrafast",
        VideoEncodingSpeed::SuperFast => "superfast",
        VideoEncodingSpeed::VeryFast => "veryfast",
        VideoEncodingSpeed::Faster => "faster",
        VideoEncodingSpeed::Fast => "fast",
        VideoEncodingSpeed::Medium => "medium",
        VideoEncodingSpeed::Slow => "slow",
        VideoEncodingSpeed::Slower => "slower",
        VideoEncodingSpeed::VerySlow => "veryslow",
    }
}

fn h264_encoding_speed_to_nvenc_preset(speed: VideoEncodingSpeed) -> &'static str {
    match speed {
        VideoEncodingSpeed::UltraFast => "p1",
        VideoEncodingSpeed::SuperFast => "p2",
        VideoEncodingSpeed::VeryFast => "p3",
        VideoEncodingSpeed::Faster | VideoEncodingSpeed::Fast | VideoEncodingSpeed::Medium => "p4",
        VideoEncodingSpeed::Slow => "p5",
        VideoEncodingSpeed::Slower => "p6",
        VideoEncodingSpeed::VerySlow => "p7",
    }
}

fn h264_encoding_speed_to_amf_quality(speed: VideoEncodingSpeed) -> &'static str {
    match speed {
        VideoEncodingSpeed::UltraFast
        | VideoEncodingSpeed::SuperFast
        | VideoEncodingSpeed::VeryFast
        | VideoEncodingSpeed::Faster
        | VideoEncodingSpeed::Fast => "speed",
        VideoEncodingSpeed::Medium | VideoEncodingSpeed::Slow => "balanced",
        VideoEncodingSpeed::Slower | VideoEncodingSpeed::VerySlow => "quality",
    }
}

pub fn run_ffmpeg_pass(
    pass: &FfmpegPass,
    input_path: &str,
    output_path: &str,
    progress_callback: &dyn Fn(f32, &str),
) -> Result<(), String> {
    let ffmpeg_path = get_ffmpeg_path();

    let mut child = Command::new(&ffmpeg_path)
        .args(&pass.arguments)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to start FFMpeg process: {:?}", e))?;

    let stderr = child
        .stderr
        .take()
        .ok_or("Failed to open stderr pipe of FFMpeg")?;
    let reader = BufReader::new(stderr);

    let duration_re = Regex::new(
        r"Duration:\s*(?P<h>[0-9]{2}):(?P<m>[0-9]{2}):(?P<s>[0-9]{2})\.(?P<ms>[0-9]{2})",
    )
    .unwrap();
    let progress_re = Regex::new(r"size=\s*(?P<sz>[0-9]+).*time=(?P<h>[0-9]{2}):(?P<m>[0-9]{2}):(?P<s>[0-9]{2})\.(?P<ms>[0-9]{2})").unwrap();

    let mut total_duration = Duration::ZERO;

    for line_res in reader.lines() {
        let line = match line_res {
            Ok(l) => l,
            Err(_) => break,
        };

        // Parse duration to know the total length
        if total_duration.is_zero() {
            if let Some(caps) = duration_re.captures(&line) {
                let h: u64 = caps["h"].parse().unwrap_or(0);
                let m: u64 = caps["m"].parse().unwrap_or(0);
                let s: u64 = caps["s"].parse().unwrap_or(0);
                let ms: u64 = caps["ms"].parse().unwrap_or(0) * 10;
                total_duration =
                    Duration::from_secs(h * 3600 + m * 60 + s) + Duration::from_millis(ms);
            }
        }

        // Parse time to compute progress percent
        if !total_duration.is_zero() {
            if let Some(caps) = progress_re.captures(&line) {
                let h: u64 = caps["h"].parse().unwrap_or(0);
                let m: u64 = caps["m"].parse().unwrap_or(0);
                let s: u64 = caps["s"].parse().unwrap_or(0);
                let ms: u64 = caps["ms"].parse().unwrap_or(0) * 10;
                let current_time =
                    Duration::from_secs(h * 3600 + m * 60 + s) + Duration::from_millis(ms);

                let percent = (current_time.as_secs_f64() / total_duration.as_secs_f64()) as f32;
                progress_callback(percent.min(1.0).max(0.0), &pass.name);
            }
        }

        // Check for error lines excluding filenames to avoid false errors
        let line_cleaned = line.replace(input_path, "").replace(output_path, "");
        if line_cleaned.contains("Exiting.")
            || line_cleaned.contains("Error")
            || line_cleaned.contains("Unsupported dimensions")
            || line_cleaned.contains("No such file or directory")
        {
            if line_cleaned.contains("Error while decoding stream")
                && line_cleaned.contains("Invalid data found when processing input")
            {
                // Ignore initial TS file frame errors
            } else {
                let _ = child.kill();
                return Err(format!("FFMpeg reported error: {}", line));
            }
        }
    }

    let status = child
        .wait()
        .map_err(|e| format!("Failed to wait for FFMpeg process: {:?}", e))?;
    if !status.success() {
        return Err(format!(
            "FFMpeg process exited with failure code: {:?}",
            status.code()
        ));
    }

    Ok(())
}
