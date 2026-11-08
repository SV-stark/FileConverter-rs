use crate::doc_convert;
use crate::error::{FileConverterError, Result};
use crate::ffmpeg;
use crate::image;
use crate::office;
use crate::path_helpers;
use crate::settings::ConversionPreset;
use crate::types::{
    FileCategory, HardwareAccelerationMode, InputPostConversionAction, OutputType,
    get_extension_category, is_preset_applicable_to_file, is_text_document_extension,
};
use parking_lot::Mutex;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

/// Name of the folder that receives inputs when a preset uses
/// `MoveInArchiveFolder`. Matches the C# original ("Conversion Archives").
pub const DEFAULT_ARCHIVE_FOLDER_NAME: &str = "Conversion Archives";

/// Start/end instants of a job, used for elapsed time and ETA display.
#[derive(Debug, Clone, Copy, Default)]
pub struct JobTiming {
    pub started: Option<Instant>,
    pub finished: Option<Instant>,
}

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
    pub progress: Arc<AtomicU32>,
    pub status: Arc<Mutex<JobStatus>>,
    /// Set when `prepare` failed so the job can report the reason instead of
    /// failing later with a generic "no output path" message.
    pub preparation_error: Option<String>,
    /// Engine chosen during `prepare` (sniffing the file is not free, so it is
    /// done once instead of again at run time).
    pub engine: Option<JobEngine>,
    /// True when the output path is the input path itself and the preset will
    /// archive or delete the input afterwards. The input is then moved aside
    /// to a temporary name while converting so the output keeps the original
    /// name (C# parity) instead of becoming "name (2).ext".
    pub replaces_input: bool,
    pub timing: Arc<Mutex<JobTiming>>,
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum JobEngine {
    Image,
    Ffmpeg,
    Word,
    Excel,
    PowerPoint,
    Oxipng,
    Ico,
    Gif,
    Epub,
    Markdown,
    Typst,
    TextDocument,
}

pub fn determine_job_engine(preset: &ConversionPreset, input_path: &str) -> JobEngine {
    let ext = Path::new(input_path)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();
    let mut category = get_extension_category(&ext);

    // Fall back to magic-byte sniffing when the extension is unknown/missing,
    // so content-valid media files still route to the correct engine.
    if category == FileCategory::Misc
        && let Ok(Some(kind)) = infer::get_from_path(input_path)
    {
        let mime = kind.mime_type();
        if mime.starts_with("image/") {
            category = FileCategory::Image;
        } else if mime.starts_with("audio/") {
            category = FileCategory::Audio;
        } else if mime.starts_with("video/") {
            category = FileCategory::Video;
        }
    }

    // --- Documents -------------------------------------------------------
    if ext == "docx" || ext == "odt" || ext == "doc" {
        return JobEngine::Word;
    }
    if ext == "xlsx" || ext == "ods" || ext == "xls" {
        return JobEngine::Excel;
    }
    if ext == "pptx" || ext == "odp" || ext == "ppt" {
        return JobEngine::PowerPoint;
    }

    if crate::types::is_ebook_extension(&ext) {
        return JobEngine::Epub;
    }
    if ext == "md" || ext == "markdown" {
        return JobEngine::Markdown;
    }
    if ext == "typ" {
        return JobEngine::Typst;
    }
    // Plain text / light markup. These used to fall through to the image engine
    // and fail with "unknown image format" for every conversion.
    if is_text_document_extension(&ext) {
        return JobEngine::TextDocument;
    }

    // --- Audio / video ---------------------------------------------------
    if category == FileCategory::Audio || category == FileCategory::Video {
        return JobEngine::Ffmpeg;
    }

    // --- Image derived outputs ------------------------------------------
    if preset.output_type == OutputType::Ico {
        return JobEngine::Ico;
    }

    if preset.output_type == OutputType::Gif {
        // Still images are encoded natively; the FFmpeg palettegen recipe
        // cannot handle single-frame sources reliably (it emits an empty
        // palette file, and `fps` yields zero frames for a still image).
        if category == FileCategory::Image || category == FileCategory::AnimatedImage {
            return JobEngine::Gif;
        }
        return JobEngine::Ffmpeg;
    }

    // OxiPNG only accepts PNG containers, so it is reserved for real PNG input.
    if preset.output_type == OutputType::Png
        && ext == "png"
        && (preset.get_setting_value("OxipngLossless").is_some()
            || preset.name.to_lowercase().contains("compress"))
    {
        return JobEngine::Oxipng;
    }

    if matches!(
        preset.output_type,
        OutputType::Pdf | OutputType::Avif | OutputType::Jpg | OutputType::Png | OutputType::Webp
    ) && (category == FileCategory::Image || ext == "pdf")
    {
        return JobEngine::Image;
    }

    // `jxl-oxide` only decodes, so JPEG XL encoding still goes through FFmpeg's
    // libjxl encoder.
    JobEngine::Ffmpeg
}

/// Runs a sequence of FFmpeg passes, reporting combined progress.
///
/// `is_cancelled` is polled between passes so a long multi-pass job (for example
/// video -> GIF, which is palettegen + paletteuse) can be aborted promptly.
fn run_ffmpeg_passes(
    passes: &[ffmpeg::FfmpegPass],
    input_path: &str,
    output_path: &str,
    weight: f32,
    is_cancelled: &dyn Fn() -> bool,
    progress_cb: &(dyn Fn(f32, &str) + Sync),
) -> Result<()> {
    if passes.is_empty() {
        return Err(FileConverterError::Invalid(
            "No FFmpeg pass was generated for this preset".to_string(),
        ));
    }

    let total_passes = passes.len();
    for (i, pass) in passes.iter().enumerate() {
        if is_cancelled() {
            for other in passes {
                if let Some(path) = &other.file_to_delete {
                    let _ = std::fs::remove_file(path);
                }
            }
            return Err(FileConverterError::Invalid("Canceled".to_string()));
        }

        if let Err(e) = ffmpeg::run_ffmpeg_pass(pass, input_path, output_path, &|percent, name| {
            let overall = (i as f32 + percent) / total_passes as f32;
            progress_cb(weight * overall, name);
        }) {
            // Clean up any intermediate file the aborted pass chain owned
            // (for example a GIF palette that was never consumed).
            for other in passes {
                if let Some(path) = &other.file_to_delete {
                    let _ = std::fs::remove_file(path);
                }
            }
            return Err(e);
        }
    }
    Ok(())
}

impl ConversionJob {
    pub fn new(id: usize, preset: ConversionPreset, input_path: String) -> Self {
        ConversionJob {
            id,
            preset,
            input_path,
            output_file_paths: Vec::new(),
            progress: Arc::new(AtomicU32::new(0.0f32.to_bits())),
            status: Arc::new(Mutex::new(JobStatus::Queue)),
            preparation_error: None,
            engine: None,
            replaces_input: false,
            timing: Arc::new(Mutex::new(JobTiming::default())),
        }
    }

    /// Time spent converting so far (or in total once finished).
    pub fn elapsed(&self) -> Option<Duration> {
        let t = *self.timing.lock();
        let start = t.started?;
        Some(
            t.finished
                .unwrap_or_else(Instant::now)
                .duration_since(start),
        )
    }

    /// Rough remaining time extrapolated from progress so far, like the C#
    /// progress window. `None` until enough progress has been made.
    pub fn estimated_remaining(&self) -> Option<Duration> {
        let t = *self.timing.lock();
        if t.finished.is_some() {
            return None;
        }
        let elapsed = t.started?.elapsed();
        let progress = self.get_progress();
        if !(0.02..1.0).contains(&progress) || elapsed < Duration::from_secs(1) {
            return None;
        }
        let total = elapsed.as_secs_f32() / progress;
        Some(Duration::from_secs_f32(
            (total - elapsed.as_secs_f32()).max(0.0),
        ))
    }

    /// True while the job is queued or converting.
    pub fn is_active(&self) -> bool {
        matches!(
            *self.status.lock(),
            JobStatus::Queue | JobStatus::Converting(_)
        )
    }

    pub fn get_progress(&self) -> f32 {
        f32::from_bits(self.progress.load(Ordering::Relaxed))
    }

    pub fn set_progress(&self, val: f32) {
        self.progress.store(val.to_bits(), Ordering::Relaxed);
    }

    /// Validates that this preset can be applied to the input file.
    ///
    /// Previously only the output/input category pair was checked, which meant a
    /// preset advertising a narrow `InputTypes` list (for example the lossless
    /// PNG compressor) silently ran on unrelated files and failed deep inside the
    /// engine.
    pub fn validate(&self) -> Result<()> {
        if !Path::new(&self.input_path).is_file() {
            return Err(FileConverterError::Invalid(format!(
                "Input file does not exist: {}",
                self.input_path
            )));
        }

        let declared: Vec<String> = self
            .preset
            .input_types
            .iter()
            .map(|s| s.to_string())
            .collect();
        if is_preset_applicable_to_file(self.preset.output_type, &declared, &self.input_path) {
            Ok(())
        } else {
            let ext = Path::new(&self.input_path)
                .extension()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_lowercase();
            Err(FileConverterError::Invalid(format!(
                "Preset '{}' does not support '.{}' input (declared inputs: {})",
                self.preset.name,
                if ext.is_empty() { "<none>" } else { &ext },
                if declared.is_empty() {
                    "any".to_string()
                } else {
                    declared.join(", ")
                }
            )))
        }
    }

    pub fn prepare(&mut self, list_index: usize, total_count: usize) -> Result<()> {
        self.preparation_error = None;
        self.replaces_input = false;

        self.validate()
            .inspect_err(|e| self.preparation_error = Some(e.to_string()))?;

        let ext = Path::new(&self.input_path)
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("");

        // Determine output files count.
        // A PDF input produces one file per page *unless* it is being rewritten
        // to PDF (the optimisation path emits a single file).
        let pdf_identity = self.preset.output_type == OutputType::Pdf;
        let engine = determine_job_engine(&self.preset, &self.input_path);
        self.engine = Some(engine);
        let count = match engine {
            JobEngine::Image if ext.eq_ignore_ascii_case("pdf") && !pdf_identity => {
                image::get_pdf_page_count(&self.input_path).unwrap_or(1)
            }
            // For Office conversion to images, it will be determined during conversion
            // dynamically, so we initialize with a placeholder of 1.
            _ => 1,
        };

        let mut paths = Vec::new();

        // When one input produces several files (pages of a PDF), `(n:i)` /
        // `(n:c)` number those files ("1 of N" .. "N of N"), as in the C#
        // original. A single-output job is numbered by its position in the
        // batch instead, which makes `(n:i)` useful for renaming batches.
        // (Mixing both used to number page 1 of the second file as "2 of N".)
        let (number_base, number_max) = if count > 1 {
            (0, count)
        } else {
            (list_index, total_count.max(1))
        };

        let archives_or_deletes_input = matches!(
            self.preset.input_post_conversion_action,
            InputPostConversionAction::MoveInArchiveFolder | InputPostConversionAction::Delete
        );

        for index in 0..count {
            // Generate templated path
            let generated = path_helpers::generate_file_path_from_template(
                &self.input_path,
                self.preset.output_type.extension(),
                &self.preset.output_file_name_template,
                number_base + index + 1,
                number_max,
            );

            if !path_helpers::is_path_valid(&generated) {
                return Err(FileConverterError::Invalid(format!(
                    "Generated output path is invalid: {}",
                    generated
                )));
            }

            // Create folders if needed
            if !path_helpers::create_folders(&generated) {
                return Err(FileConverterError::Invalid(
                    "Failed to create output directory folders".to_string(),
                ));
            }

            // The input itself sits on the generated path. If it is going to be
            // archived or deleted anyway, keep the name: the input is moved aside
            // while converting (see `run`).
            if count == 1
                && archives_or_deletes_input
                && path_helpers::is_same_path(&generated, &self.input_path)
            {
                self.replaces_input = true;
                paths.push(generated);
                continue;
            }

            // Generate unique path to avoid collisions
            let unique = path_helpers::generate_unique_path(&generated, &paths);
            paths.push(unique.to_string_lossy().to_string());
        }

        self.output_file_paths = paths;
        Ok(())
    }

    pub fn cancel(&self) {
        let mut status = self.status.lock();
        if *status == JobStatus::Queue || matches!(*status, JobStatus::Converting(_)) {
            *status = JobStatus::Canceled;
        }
    }

    /// True once `cancel` has been requested.
    ///
    /// Engines poll this at their own safe points (between FFmpeg passes, between
    /// rendered PDF pages) so the Cancel button takes effect promptly instead of
    /// only after the whole job has finished.
    pub fn is_cancelled(&self) -> bool {
        *self.status.lock() == JobStatus::Canceled
    }

    pub fn run(&self, hw_accel: HardwareAccelerationMode) {
        {
            let mut status = self.status.lock();
            if *status == JobStatus::Canceled {
                return;
            }
            *status = JobStatus::Converting("Preparing".to_string());
        }
        self.timing.lock().started = Some(Instant::now());
        let _finish = scopeguard::guard((), |_| {
            self.timing.lock().finished = Some(Instant::now());
        });

        // A job whose preparation was rejected must not run: it has no output
        // paths and would otherwise fail with a misleading message.
        if let Some(err) = &self.preparation_error {
            *self.status.lock() = JobStatus::Failed(err.clone());
            return;
        }

        // Output replaces the input: move the input aside first so the engine
        // can write the output under the original name.
        let moved_input = if self.replaces_input {
            let path = Path::new(&self.input_path);
            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("input");
            let ext = path
                .extension()
                .and_then(|s| s.to_str())
                .map(|e| format!(".{e}"))
                .unwrap_or_default();
            let temp = path_helpers::generate_unique_path(
                path.with_file_name(format!("{stem}_TEMP{ext}")),
                &[],
            );
            if let Err(e) = std::fs::rename(&self.input_path, &temp) {
                *self.status.lock() =
                    JobStatus::Failed(format!("Could not move the input file aside: {e}"));
                return;
            }
            Some(temp.to_string_lossy().to_string())
        } else {
            None
        };
        let source = moved_input.as_deref().unwrap_or(&self.input_path);
        let restore_input = || {
            if let Some(temp) = &moved_input {
                let _ = std::fs::rename(temp, &self.input_path);
            }
        };

        let progress_clone = self.progress.clone();
        let status_clone = self.status.clone();

        let progress_cb = move |percent: f32, msg: &str| {
            progress_clone.store(percent.to_bits(), Ordering::Relaxed);
            let mut s = status_clone.lock();
            if let JobStatus::Converting(_) = *s {
                *s = JobStatus::Converting(msg.to_string());
            }
        };

        let result = self.execute(source, &progress_cb, hw_accel);

        let mut status = self.status.lock();
        if *status == JobStatus::Canceled {
            // Delete output files
            for path in &self.output_file_paths {
                let _ = std::fs::remove_file(path);
            }
            restore_input();
            return;
        }

        match result {
            Ok(_) => {
                // Only report success for outputs that actually exist. Multi-pass
                // engines (or page-count guesses) can leave gaps.
                let missing: Vec<&String> = self
                    .output_file_paths
                    .iter()
                    .filter(|p| !Path::new(p).is_file())
                    .collect();

                if !missing.is_empty() {
                    *status = JobStatus::Failed(format!(
                        "Conversion reported success but {} output file(s) are missing",
                        missing.len()
                    ));
                    for path in &self.output_file_paths {
                        let _ = std::fs::remove_file(path);
                    }
                    restore_input();
                    return;
                }

                *status = JobStatus::Done;
                self.progress.store(1.0f32.to_bits(), Ordering::Relaxed);

                // Copy timestamps (created / accessed / modified) from the input.
                self.sync_file_timestamps(source);

                // Apply post conversion action
                if let Err(e) = self.apply_post_conversion_action(source) {
                    tracing::warn!(
                        "Post-conversion action failed for {}: {}",
                        self.input_path,
                        e
                    );
                    // Never leave the input stranded under its temporary name.
                    restore_input();
                }
            }
            Err(e) => {
                // A cancel request that raced the engine is reported as
                // `Canceled`, not as a failure with a confusing message.
                // `status` is already locked here, so it must be inspected
                // directly (`is_cancelled` would re-lock and deadlock).
                if *status == JobStatus::Canceled {
                    *status = JobStatus::Canceled;
                } else {
                    *status = JobStatus::Failed(e.to_string());
                }
                // Delete output files on failure
                for path in &self.output_file_paths {
                    let _ = std::fs::remove_file(path);
                }
                restore_input();
            }
        }
    }

    fn execute(
        &self,
        source: &str,
        progress_cb: &(dyn Fn(f32, &str) + Sync),
        hw_accel: HardwareAccelerationMode,
    ) -> Result<()> {
        let out_path = self.output_file_paths.first().ok_or_else(|| {
            FileConverterError::Invalid("No output path specified for job".to_string())
        })?;
        let engine = self
            .engine
            .unwrap_or_else(|| determine_job_engine(&self.preset, source));

        match engine {
            JobEngine::Epub => doc_convert::run_epub_conversion(
                source,
                out_path,
                self.preset.output_type,
                progress_cb,
            ),
            JobEngine::Markdown => doc_convert::run_markdown_conversion(
                source,
                out_path,
                self.preset.output_type,
                progress_cb,
            ),
            JobEngine::Typst => doc_convert::run_typst_conversion(
                source,
                out_path,
                self.preset.output_type,
                progress_cb,
            ),
            JobEngine::TextDocument => doc_convert::run_text_document_conversion(
                source,
                out_path,
                self.preset.output_type,
                progress_cb,
            ),
            JobEngine::Oxipng => {
                image::run_oxipng_compression(Some(&self.preset), source, out_path, progress_cb)
            }
            JobEngine::Ico => {
                // ICO is written natively (multi-resolution, aspect preserved).
                image::run_image_conversion(
                    &self.preset,
                    source,
                    std::slice::from_ref(out_path),
                    progress_cb,
                )
            }
            JobEngine::Gif => {
                if image::source_is_animated(source) {
                    // Animated images keep the two-pass palettegen/paletteuse
                    // recipe, which needs real frame timing to work.
                    let passes =
                        ffmpeg::get_ffmpeg_passes(&self.preset, source, out_path, hw_accel)?;
                    run_ffmpeg_passes(
                        &passes,
                        source,
                        out_path,
                        1.0,
                        &|| self.is_cancelled(),
                        progress_cb,
                    )
                } else {
                    image::run_still_gif_conversion(&self.preset, source, out_path, progress_cb)
                }
            }
            JobEngine::Image => {
                if self.preset.output_type == OutputType::Pdf
                    && source.to_lowercase().ends_with(".pdf")
                {
                    progress_cb(0.1, "Optimizing PDF");
                    let dpi = self
                        .preset
                        .get_setting_value("PdfTargetDpi")
                        .and_then(|v| v.trim().parse::<u32>().ok())
                        .filter(|d| *d > 0)
                        .unwrap_or(150);
                    let options = crate::pdf_compress::PdfCompressOptions {
                        target_dpi: dpi,
                        jpeg_quality: self
                            .preset
                            .get_setting_value("PdfJpegQuality")
                            .and_then(|v| v.trim().parse::<u8>().ok())
                            .filter(|q| *q > 0)
                            .unwrap_or(75),
                    };
                    let res = crate::pdf_compress::compress_pdf(source, out_path, &options);
                    progress_cb(1.0, "Complete");
                    res
                } else {
                    image::run_image_conversion(
                        &self.preset,
                        source,
                        &self.output_file_paths,
                        progress_cb,
                    )
                }
            }
            JobEngine::Word => office::run_office_conversion(
                &self.preset,
                "winword.exe",
                source,
                &self.output_file_paths,
                progress_cb,
            ),
            JobEngine::Excel => office::run_office_conversion(
                &self.preset,
                "excel.exe",
                source,
                &self.output_file_paths,
                progress_cb,
            ),
            JobEngine::PowerPoint => office::run_office_conversion(
                &self.preset,
                "powerpnt.exe",
                source,
                &self.output_file_paths,
                progress_cb,
            ),
            JobEngine::Ffmpeg => {
                let passes = ffmpeg::get_ffmpeg_passes(&self.preset, source, out_path, hw_accel)?;
                run_ffmpeg_passes(
                    &passes,
                    source,
                    out_path,
                    1.0,
                    &|| self.is_cancelled(),
                    progress_cb,
                )
            }
        }
    }

    /// Copies the input's creation, access and modification times onto every
    /// output, as the C# original does. Creation time used to be skipped
    /// ("cannot be set portably"), but `std` supports it on Windows.
    fn sync_file_timestamps(&self, source: &str) {
        let Ok(metadata) = std::fs::metadata(source) else {
            return;
        };

        let mut times = std::fs::FileTimes::new();
        if let Ok(t) = metadata.accessed() {
            times = times.set_accessed(t);
        }
        if let Ok(t) = metadata.modified() {
            times = times.set_modified(t);
        }
        #[cfg(windows)]
        if let Ok(t) = metadata.created() {
            use std::os::windows::fs::FileTimesExt;
            times = times.set_created(t);
        }

        for path in &self.output_file_paths {
            if let Ok(file) = std::fs::OpenOptions::new().write(true).open(path) {
                let _ = file.set_times(times);
            }
        }
    }

    fn apply_post_conversion_action(&self, source: &str) -> Result<()> {
        match self.preset.input_post_conversion_action {
            InputPostConversionAction::None => Ok(()),
            InputPostConversionAction::MoveInArchiveFolder => {
                // The archived copy keeps the *original* name even when the
                // input was moved aside to a temporary name.
                let input_path = Path::new(&self.input_path);
                let parent = input_path.parent().ok_or_else(|| {
                    FileConverterError::Invalid("No parent folder found".to_string())
                })?;
                let file_name = input_path
                    .file_name()
                    .ok_or_else(|| FileConverterError::Invalid("No file name found".to_string()))?;

                // Folder name: default is "Archive" or from preset settings
                let archive_folder_name = self
                    .preset
                    .get_setting_value("ConversionArchiveFolderName")
                    .filter(|name| !name.trim().is_empty())
                    .unwrap_or(DEFAULT_ARCHIVE_FOLDER_NAME);
                let archive_dir = parent.join(archive_folder_name);

                if !archive_dir.exists() {
                    std::fs::create_dir_all(&archive_dir)?;
                }

                let target_path =
                    path_helpers::generate_unique_path(archive_dir.join(file_name), &[]);
                std::fs::rename(source, target_path).map_err(FileConverterError::Io)?;
                Ok(())
            }
            InputPostConversionAction::Delete => {
                trash::delete(source).map_err(|e| {
                    FileConverterError::Io(std::io::Error::other(format!(
                        "Failed to move file to Recycle Bin: {:?}",
                        e
                    )))
                })?;
                Ok(())
            }
        }
    }
}

#[cfg(target_os = "windows")]
pub fn copy_files_to_clipboard(paths: &[String]) -> Result<()> {
    use clipboard_win::raw;

    if paths.is_empty() {
        return Ok(());
    }

    // `clipboard_win::raw::set_file_list` deliberately uses `NoClear`, so the
    // clipboard must be opened (and closed) by the caller - otherwise
    // `SetClipboardData` fails with ERROR_CLIPBOARD_NOT_OPEN and nothing is
    // ever copied.
    raw::open().map_err(|e| clipboard_error("Failed to open clipboard", e))?;

    let set_result = raw::empty()
        .and_then(|_| raw::set_file_list(paths))
        .map_err(|e| clipboard_error("Failed to copy to clipboard", e));

    // Always release the clipboard, even when setting the data failed.
    let _ = raw::close();

    set_result
}

#[cfg(target_os = "windows")]
fn clipboard_error(context: &str, err: impl std::fmt::Debug) -> FileConverterError {
    FileConverterError::Io(std::io::Error::other(format!("{}: {:?}", context, err)))
}

#[cfg(not(target_os = "windows"))]
pub fn copy_files_to_clipboard(_paths: &[String]) -> Result<()> {
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

    /// Number of worker threads to use for this batch.
    pub fn concurrency(&self) -> usize {
        let configured = if self.max_threads == 0 {
            // Same default as the C# original: half the logical CPUs.
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
        configured.clamp(1, self.jobs.len().max(1))
    }

    /// Requests cancellation of every job that has not finished yet.
    pub fn cancel_all(&self) {
        for job in &self.jobs {
            job.cancel();
        }
    }

    pub fn execute_all(&self) {
        // Workers pull jobs strictly in list order (like the C# queue), so
        // the progress window fills from the top instead of jumping around as
        // rayon's recursive splitting did. Engines that parallelise internally
        // (PDF page rendering, image recompression) still use the global rayon
        // pool, and no throw-away thread pool is built per batch.
        let next = AtomicUsize::new(0);
        let workers = self.concurrency();
        thread::scope(|scope| {
            for _ in 0..workers {
                scope.spawn(|| {
                    loop {
                        let index = next.fetch_add(1, Ordering::Relaxed);
                        let Some(job) = self.jobs.get(index) else {
                            break;
                        };
                        // A panic inside one engine must not take the whole batch
                        // (and the progress window) down with it.
                        let outcome =
                            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                job.run(self.hw_accel)
                            }));
                        if let Err(panic) = outcome {
                            let reason = panic
                                .downcast_ref::<&str>()
                                .map(|s| s.to_string())
                                .or_else(|| panic.downcast_ref::<String>().cloned())
                                .unwrap_or_else(|| "unknown error".to_string());
                            *job.status.lock() =
                                JobStatus::Failed(format!("Internal error: {reason}"));
                            job.timing.lock().finished = Some(Instant::now());
                            for path in &job.output_file_paths {
                                let _ = std::fs::remove_file(path);
                            }
                        }
                    }
                });
            }
        });

        // Copy files to clipboard on completion.
        // Only files that actually exist are offered.
        if self.copy_clipboard {
            let mut successful_files = Vec::new();
            for job in &self.jobs {
                let is_done = matches!(*job.status.lock(), JobStatus::Done);
                if is_done {
                    for path in &job.output_file_paths {
                        if Path::new(path).is_file() {
                            successful_files.push(path.clone());
                        }
                    }
                }
            }
            if !successful_files.is_empty()
                && let Err(e) = copy_files_to_clipboard(&successful_files)
            {
                tracing::warn!("Failed to copy results to the clipboard: {}", e);
            }
        }
    }
}
