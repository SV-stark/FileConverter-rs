use crate::cda;
use crate::ffmpeg;
use crate::image;
use crate::office;
use crate::path_helpers;
use crate::settings::ConversionPreset;
use crate::types::{HardwareAccelerationMode, InputPostConversionAction, OutputType};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobStatus {
    Queue,
    Converting(String), // Status message
    Done,
    Failed(String), // Error message
    Canceled,
}

#[derive(Debug, Clone)]
pub struct ConversionJob {
    pub id: usize,
    pub preset: ConversionPreset,
    pub input_path: String,
    pub output_file_paths: Vec<String>,
    pub progress: Arc<Mutex<f32>>,
    pub status: Arc<Mutex<JobStatus>>,
    pub is_cda: bool,
}

pub enum JobEngine {
    Cda,
    Word,
    Excel,
    PowerPoint,
    Ico,
    Gif,
    Image,
    Ffmpeg,
}

pub fn determine_job_engine(preset: &ConversionPreset, input_path: &str) -> JobEngine {
    let ext = Path::new(input_path)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();

    if ext == "cda" {
        return JobEngine::Cda;
    }

    if ext == "docx" || ext == "odt" || ext == "doc" {
        return JobEngine::Word;
    }

    if ext == "xlsx" || ext == "ods" || ext == "xls" {
        return JobEngine::Excel;
    }

    if ext == "pptx" || ext == "odp" || ext == "ppt" {
        return JobEngine::PowerPoint;
    }

    if preset.output_type == OutputType::Ico {
        return JobEngine::Ico;
    }

    if preset.output_type == OutputType::Gif {
        return JobEngine::Gif;
    }

    if preset.output_type == OutputType::Pdf
        || preset.output_type == OutputType::Avif
        || preset.output_type == OutputType::Jpg
        || preset.output_type == OutputType::Png
        || preset.output_type == OutputType::Webp
    {
        return JobEngine::Image;
    }

    JobEngine::Ffmpeg
}

// Replicate C# Helpers.GetExtensionCategory mapping
fn get_extension_category(ext: &str) -> &'static str {
    match ext {
        "aac" | "aiff" | "ape" | "cda" | "flac" | "mp3" | "m4a" | "m4b" | "oga" | "ogg"
        | "opus" | "wav" | "wma" => "Audio",
        "3gp" | "3gpp" | "avi" | "bik" | "flv" | "m4v" | "mp4" | "mpg" | "mpeg" | "mov" | "mkv"
        | "ogv" | "rm" | "ts" | "vob" | "webm" | "wmv" => "Video",
        "arw" | "avif" | "bmp" | "cr2" | "dds" | "dng" | "exr" | "heic" | "ico" | "jfif"
        | "jpg" | "jpeg" | "nef" | "png" | "psd" | "raf" | "tga" | "tif" | "tiff" | "svg"
        | "xcf" | "webp" => "Image",
        "gif" => "Animated Image",
        "pdf" | "doc" | "docx" | "ppt" | "pptx" | "odp" | "ods" | "odt" | "xls" | "xlsx" => {
            "Document"
        }
        _ => "Misc",
    }
}

// Replicate compatibility check
fn is_output_type_compatible_with_category(output_type: OutputType, category: &str) -> bool {
    if category == "Misc" {
        return true;
    }
    match output_type {
        OutputType::Aac
        | OutputType::Flac
        | OutputType::Mp3
        | OutputType::Ogg
        | OutputType::Wav => category == "Audio" || category == "Video",
        OutputType::Avi
        | OutputType::Mkv
        | OutputType::Mp4
        | OutputType::Ogv
        | OutputType::Webm => category == "Video" || category == "Animated Image",
        OutputType::Avif
        | OutputType::Ico
        | OutputType::Jpg
        | OutputType::Png
        | OutputType::Webp => {
            category == "Image" || category == "Document" || category == "Animated Image"
        }
        OutputType::Gif => {
            category == "Image" || category == "Video" || category == "Animated Image"
        }
        OutputType::Pdf => category == "Image" || category == "Document",
        OutputType::None => false,
    }
}

impl ConversionJob {
    pub fn new(id: usize, preset: ConversionPreset, input_path: String) -> Self {
        let is_cda = Path::new(&input_path)
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_lowercase()
            == "cda";

        ConversionJob {
            id,
            preset,
            input_path,
            output_file_paths: Vec::new(),
            progress: Arc::new(Mutex::new(0.0)),
            status: Arc::new(Mutex::new(JobStatus::Queue)),
            is_cda,
        }
    }

    pub fn prepare(&mut self, list_index: usize, total_count: usize) -> Result<(), String> {
        let ext = Path::new(&self.input_path)
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("");
        let category = get_extension_category(&ext.to_lowercase());

        if !is_output_type_compatible_with_category(self.preset.output_type, category) {
            return Err("Input file type is incompatible with output file type".to_string());
        }

        // Determine output files count
        let count = match determine_job_engine(&self.preset, &self.input_path) {
            JobEngine::Image if ext.to_lowercase() == "pdf" => {
                image::get_pdf_page_count(&self.input_path).unwrap_or(1)
            }
            // For Office conversion to images, it will be determined during conversion
            // dynamically, so we initialize with a placeholder of 1.
            _ => 1,
        };

        let mut paths = Vec::new();
        for index in 0..count {
            let out_path = self.preset.output_file_name_template.clone();

            // Generate templated path
            let generated = path_helpers::generate_file_path_from_template(
                &self.input_path,
                &format!("{:?}", self.preset.output_type),
                &out_path,
                list_index + index + 1,
                total_count,
            );

            if !path_helpers::is_path_valid(&generated) {
                return Err("Generated output path is invalid".to_string());
            }

            // Create folders if needed
            if !path_helpers::create_folders(&generated) {
                return Err("Failed to create output directory folders".to_string());
            }

            // Generate unique path to avoid collisions
            let unique = path_helpers::generate_unique_path(&generated, &paths);
            paths.push(unique.to_string_lossy().to_string());
        }

        self.output_file_paths = paths;
        Ok(())
    }

    pub fn cancel(&self) {
        let mut status = self.status.lock().unwrap();
        if *status == JobStatus::Queue || matches!(*status, JobStatus::Converting(_)) {
            *status = JobStatus::Canceled;
        }
    }

    pub fn run(&self, hw_accel: HardwareAccelerationMode) {
        {
            let mut status = self.status.lock().unwrap();
            if *status == JobStatus::Canceled {
                return;
            }
            *status = JobStatus::Converting("Preparing".to_string());
        }

        let progress_clone = self.progress.clone();
        let status_clone = self.status.clone();

        let progress_cb = move |percent: f32, msg: &str| {
            let mut p = progress_clone.lock().unwrap();
            *p = percent;
            let mut s = status_clone.lock().unwrap();
            if let JobStatus::Converting(_) = *s {
                *s = JobStatus::Converting(msg.to_string());
            }
        };

        let result = self.execute(&progress_cb, hw_accel);

        let mut status = self.status.lock().unwrap();
        if *status == JobStatus::Canceled {
            // Delete output files
            for path in &self.output_file_paths {
                let _ = std::fs::remove_file(path);
            }
            return;
        }

        match result {
            Ok(_) => {
                *status = JobStatus::Done;
                let mut p = self.progress.lock().unwrap();
                *p = 1.0;

                // Copy timestamp from input file to output files
                self.sync_file_timestamps();

                // Apply post conversion action
                let _ = self.apply_post_conversion_action();
            }
            Err(e) => {
                *status = JobStatus::Failed(e);
                // Delete output files on failure
                for path in &self.output_file_paths {
                    let _ = std::fs::remove_file(path);
                }
            }
        }
    }

    fn execute(
        &self,
        progress_cb: &(dyn Fn(f32, &str) + Sync),
        hw_accel: HardwareAccelerationMode,
    ) -> Result<(), String> {
        let engine = determine_job_engine(&self.preset, &self.input_path);

        match engine {
            JobEngine::Cda => {
                let track = path_helpers::get_cda_track_number(&self.input_path)
                    .ok_or_else(|| "Failed to parse track number from CDA file name".to_string())?;
                let drive_str = path_helpers::get_path_drive_letter(&self.input_path)
                    .ok_or_else(|| "Failed to retrieve drive letter".to_string())?;
                let drive_char = drive_str.chars().next().unwrap();

                let temp_dir = std::env::temp_dir();
                let temp_wav = path_helpers::generate_unique_path(
                    temp_dir.join(format!("Track{}_temp.wav", track)),
                    &[],
                );
                let temp_wav_str = temp_wav.to_string_lossy().to_string();

                // 1. Extract CDA to temp WAV
                cda::extract_cda_track(drive_char, track, &temp_wav_str, progress_cb)?;

                // 2. Compress temp WAV to final output
                // Create a sub-preset for WAV conversion
                let sub_preset = self.preset.clone();
                // Ensure we don't recursive CDA loop
                let conversion_res = if preset_uses_ffmpeg(&sub_preset) {
                    let passes = ffmpeg::get_ffmpeg_passes(
                        &sub_preset,
                        &temp_wav_str,
                        &self.output_file_paths[0],
                        hw_accel,
                    )?;
                    let total_passes = passes.len();
                    for (i, pass) in passes.iter().enumerate() {
                        ffmpeg::run_ffmpeg_pass(
                            pass,
                            &temp_wav_str,
                            &self.output_file_paths[0],
                            &|percent, name| {
                                let overall = (i as f32 + percent) / total_passes as f32;
                                progress_cb(overall, name);
                            },
                        )?;
                    }
                    Ok(())
                } else {
                    image::run_image_conversion(
                        &sub_preset,
                        &temp_wav_str,
                        &self.output_file_paths,
                        progress_cb,
                    )
                };

                let _ = std::fs::remove_file(temp_wav);
                conversion_res
            }
            JobEngine::Ico => {
                let temp_dir = std::env::temp_dir();
                let file_name = Path::new(&self.input_path)
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("temp");
                let temp_png = path_helpers::generate_unique_path(
                    temp_dir.join(format!("{}_ico_temp.png", file_name)),
                    &[],
                );
                let temp_png_str = temp_png.to_string_lossy().to_string();

                // 1. Convert input to intermediate clamped PNG
                let mut png_preset = self.preset.clone();
                png_preset.output_type = OutputType::Png;
                png_preset.set_setting_value("ImageClampSizePowerOf2", "True");
                png_preset.set_setting_value("ImageMaximumSize", "256");

                image::run_image_conversion(
                    &png_preset,
                    &self.input_path,
                    &[temp_png_str.clone()],
                    &|percent, _| {
                        progress_cb(percent * 0.5, "Resizing");
                    },
                )?;

                // 2. Convert PNG to ICO
                let passes = ffmpeg::get_ffmpeg_passes(
                    &self.preset,
                    &temp_png_str,
                    &self.output_file_paths[0],
                    hw_accel,
                )?;
                let res = ffmpeg::run_ffmpeg_pass(
                    &passes[0],
                    &temp_png_str,
                    &self.output_file_paths[0],
                    &|percent, name| {
                        progress_cb(0.5 + percent * 0.5, name);
                    },
                );

                let _ = std::fs::remove_file(temp_png);
                res
            }
            JobEngine::Gif => {
                let ext = Path::new(&self.input_path)
                    .extension()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_lowercase();
                let is_image = get_extension_category(&ext) == "Image";

                if is_image && ext != "png" {
                    // Convert to PNG first
                    let temp_dir = std::env::temp_dir();
                    let file_name = Path::new(&self.input_path)
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or("temp");
                    let temp_png = path_helpers::generate_unique_path(
                        temp_dir.join(format!("{}_gif_temp.png", file_name)),
                        &[],
                    );
                    let temp_png_str = temp_png.to_string_lossy().to_string();

                    let mut png_preset = self.preset.clone();
                    png_preset.output_type = OutputType::Png;

                    image::run_image_conversion(
                        &png_preset,
                        &self.input_path,
                        &[temp_png_str.clone()],
                        &|percent, _| {
                            progress_cb(percent * 0.3, "Pre-processing");
                        },
                    )?;

                    let passes = ffmpeg::get_ffmpeg_passes(
                        &self.preset,
                        &temp_png_str,
                        &self.output_file_paths[0],
                        hw_accel,
                    )?;
                    let total_passes = passes.len();
                    for (i, pass) in passes.iter().enumerate() {
                        let step_res = ffmpeg::run_ffmpeg_pass(
                            pass,
                            &temp_png_str,
                            &self.output_file_paths[0],
                            &|percent, name| {
                                let overall =
                                    0.3 + (i as f32 + percent) / total_passes as f32 * 0.7;
                                progress_cb(overall, name);
                            },
                        );

                        if step_res.is_err() {
                            let _ = std::fs::remove_file(&temp_png);
                            return step_res;
                        }
                    }
                    let _ = std::fs::remove_file(temp_png);
                } else {
                    let passes = ffmpeg::get_ffmpeg_passes(
                        &self.preset,
                        &self.input_path,
                        &self.output_file_paths[0],
                        hw_accel,
                    )?;
                    let total_passes = passes.len();
                    for (i, pass) in passes.iter().enumerate() {
                        ffmpeg::run_ffmpeg_pass(
                            pass,
                            &self.input_path,
                            &self.output_file_paths[0],
                            &|percent, name| {
                                let overall = (i as f32 + percent) / total_passes as f32;
                                progress_cb(overall, name);
                            },
                        )?;
                    }
                }
                Ok(())
            }
            JobEngine::Image => image::run_image_conversion(
                &self.preset,
                &self.input_path,
                &self.output_file_paths,
                progress_cb,
            ),
            JobEngine::Word => office::run_office_conversion(
                &self.preset,
                "winword.exe",
                &self.input_path,
                &self.output_file_paths,
                progress_cb,
            ),
            JobEngine::Excel => office::run_office_conversion(
                &self.preset,
                "excel.exe",
                &self.input_path,
                &self.output_file_paths,
                progress_cb,
            ),
            JobEngine::PowerPoint => office::run_office_conversion(
                &self.preset,
                "powerpnt.exe",
                &self.input_path,
                &self.output_file_paths,
                progress_cb,
            ),
            JobEngine::Ffmpeg => {
                let passes = ffmpeg::get_ffmpeg_passes(
                    &self.preset,
                    &self.input_path,
                    &self.output_file_paths[0],
                    hw_accel,
                )?;
                let total_passes = passes.len();
                for (i, pass) in passes.iter().enumerate() {
                    ffmpeg::run_ffmpeg_pass(
                        pass,
                        &self.input_path,
                        &self.output_file_paths[0],
                        &|percent, name| {
                            let overall = (i as f32 + percent) / total_passes as f32;
                            progress_cb(overall, name);
                        },
                    )?;
                }
                Ok(())
            }
        }
    }

    fn sync_file_timestamps(&self) {
        if let Ok(metadata) = std::fs::metadata(&self.input_path) {
            let _creation_time = metadata
                .created()
                .unwrap_or_else(|_| std::time::SystemTime::now());
            let accessed_time = metadata
                .accessed()
                .unwrap_or_else(|_| std::time::SystemTime::now());
            let modified_time = metadata
                .modified()
                .unwrap_or_else(|_| std::time::SystemTime::now());

            for path in &self.output_file_paths {
                let _ = filetime::set_file_times(
                    path,
                    filetime::FileTime::from_system_time(accessed_time),
                    filetime::FileTime::from_system_time(modified_time),
                );
                // Windows specific creation time setting
                #[cfg(target_os = "windows")]
                {
                    // If creation time is available, we can set it via filetime or raw win32,
                    // filetime crate handles modification and access.
                }
            }
        }
    }

    fn apply_post_conversion_action(&self) -> Result<(), String> {
        match self.preset.input_post_conversion_action {
            InputPostConversionAction::None => Ok(()),
            InputPostConversionAction::MoveInArchiveFolder => {
                let input_path = Path::new(&self.input_path);
                let parent = input_path.parent().ok_or("No parent folder found")?;
                let file_name = input_path.file_name().ok_or("No file name found")?;

                // Folder name: default is "Archive" or from preset settings
                let archive_folder_name = self
                    .preset
                    .get_setting_value("ConversionArchiveFolderName")
                    .unwrap_or("Archive");
                let archive_dir = parent.join(archive_folder_name);

                if !archive_dir.exists() {
                    std::fs::create_dir_all(&archive_dir).map_err(|e| e.to_string())?;
                }

                let target_path =
                    path_helpers::generate_unique_path(archive_dir.join(file_name), &[]);
                std::fs::rename(input_path, target_path)
                    .map_err(|e| format!("Failed to move file to archive: {:?}", e))?;
                Ok(())
            }
            InputPostConversionAction::Delete => {
                std::fs::remove_file(&self.input_path)
                    .map_err(|e| format!("Failed to delete input file: {:?}", e))?;
                Ok(())
            }
        }
    }
}

fn preset_uses_ffmpeg(preset: &ConversionPreset) -> bool {
    match preset.output_type {
        OutputType::Aac
        | OutputType::Avi
        | OutputType::Flac
        | OutputType::Mp3
        | OutputType::Mkv
        | OutputType::Mp4
        | OutputType::Ogg
        | OutputType::Ogv
        | OutputType::Wav
        | OutputType::Webm => true,
        _ => false,
    }
}

#[cfg(target_os = "windows")]
pub fn copy_files_to_clipboard(paths: &[String]) -> Result<(), String> {
    use clipboard_win::raw::set_file_list;
    set_file_list(paths).map_err(|e| format!("Failed to copy to clipboard: {:?}", e))?;
    Ok(())
}

#[cfg(not(target_os = "windows"))]
pub fn copy_files_to_clipboard(_paths: &[String]) -> Result<(), String> {
    Ok(())
}

// Thread pool based Scheduler
pub struct ConversionScheduler {
    pub jobs: Vec<ConversionJob>,
    pub max_threads: usize,
    pub hw_accel: HardwareAccelerationMode,
    pub copy_clipboard: bool,
}

impl ConversionScheduler {
    pub fn new(
        jobs: Vec<ConversionJob>,
        max_threads: usize,
        hw_accel: HardwareAccelerationMode,
        copy_clipboard: bool,
    ) -> Self {
        ConversionScheduler {
            jobs,
            max_threads,
            hw_accel,
            copy_clipboard,
        }
    }

    pub fn execute_all(&self) {
        let max_concurrency = if self.max_threads == 0 {
            std::cmp::max(
                1,
                thread::available_parallelism()
                    .map(|n| n.get())
                    .unwrap_or(2)
                    / 2,
            )
        } else {
            self.max_threads
        };

        let (tx, rx) = std::sync::mpsc::channel::<(usize, ConversionJob)>();
        let rx = Arc::new(Mutex::new(rx));

        for (idx, job) in self.jobs.iter().enumerate() {
            let _ = tx.send((idx, job.clone()));
        }
        drop(tx); // Close queue so workers terminate when finished

        let cda_mutex = Arc::new(Mutex::new(()));

        let mut handles = Vec::new();
        for _ in 0..max_concurrency {
            let rx = rx.clone();
            let cda_mutex = cda_mutex.clone();
            let hw_accel = self.hw_accel;

            let handle = thread::spawn(move || {
                while let Ok((_, job)) = {
                    let lock = rx.lock().unwrap();
                    lock.recv()
                } {
                    if job.is_cda {
                        let _cda_guard = cda_mutex.lock().unwrap();
                        job.run(hw_accel);
                    } else {
                        job.run(hw_accel);
                    }
                }
            });
            handles.push(handle);
        }

        for h in handles {
            let _ = h.join();
        }

        // Copy files to clipboard on completion
        if self.copy_clipboard {
            let mut successful_files = Vec::new();
            for job in &self.jobs {
                let status = job.status.lock().unwrap();
                if *status == JobStatus::Done {
                    for path in &job.output_file_paths {
                        successful_files.push(path.clone());
                    }
                }
            }
            if !successful_files.is_empty() {
                let _ = copy_files_to_clipboard(&successful_files);
            }
        }
    }
}
