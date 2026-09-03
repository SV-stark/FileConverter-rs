# Changelog

All notable changes to **FileConverter-rs** will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [0.9.5] - 2026-09-03

### 🛡️ Security, Reliability & Critical Fixes
- **PowerShell Injection & COM Leak Elimination (`office.rs`):**
  - Switched Office conversion execution to UTF-16LE Base64 `-EncodedCommand`, preventing command injection via special characters (`;`, `$`, quotes) in filenames.
  - Wrapped COM script automation in `try/finally` blocks ensuring `$doc.Close(0)` and `$app.Quit()` always execute, eliminating orphaned `WINWORD.EXE`, `EXCEL.EXE`, and `POWERPNT.EXE` background processes.
- **Panic Prevention on Worker Execution (`scheduler.rs` & `path_helpers.rs`):**
  - Replaced direct unchecked `output_file_paths[0]` indexing with safe extraction, returning clear errors instead of worker thread panics.
  - Replaced byte slicing with UTF-8 boundary safe `rfind('.')` in `generate_file_path_from_template`, preventing panics when processing non-ASCII or multi-byte filenames.
  - Updated `RE_VALID_PATH` regex to accept relative paths alongside absolute Windows and UNC paths.
- **Collision & Hijack Hardening (`ffmpeg.rs`, `ffmpeg_download.rs`, `main.rs`):**
  - Replaced predictable filenames (`{stem} - palette.png`, `ffmpeg_temp.zip`) with cryptographically secure temporary files via `tempfile::Builder`.
  - Replaced `cmd /c start <url>` with Win32 `ShellExecuteW`, restricted to `http(s)://` protocols.

### 🎨 GUI & UX Architecture Improvements
- **Preset Customization Persistence (`main.rs`):**
  - Wired `edit_output_type` and `edit_post_action` in `on_preset_field_changed` so user changes to output type and post-conversion action persist reliably.
  - Guarded pending file clearance to prevent silent file drop discards when presets are unselected.
  - Synchronized `exit_delay_seconds` on dashboard initialization and save.
- **Live Preset Search (`appwindow.slint`, `main.rs`):**
  - Connected the preset search bar to a reactive `search_query_changed` callback, enabling live filtering by preset name, category, and file extensions.
- **Event Loop Decoupling (`main.rs`):**
  - Replaced reentrant `window.run()` calls from settings callbacks with detached process spawning (`spawn_conversion_process`), eliminating nested event loop freezes.

### ⚡ Engine & Shell Enhancements
- **PDF Color Space & Transparency Preservation (`pdf_compress.rs`):**
  - Retained Grayscale, CMYK, and transparent `/SMask` image streams without forcing `DeviceRGB` and `DCTDecode`.
  - Added `is_encrypted()` checks and atomic same-file overwrite protection via temporary staging.
  - Corrected US Letter dimensions to use separate width and height DPI scaling factors.
- **Non-ASCII Text Preservation (`doc_convert.rs`):**
  - Replaced ASCII `< 128` filtering with `sanitize_text_for_pdf`, preserving Latin-1 accented characters (é, ü, ñ, à, ç, etc.) and typography.
  - Ensured destination directories are automatically created before writing outputs.
  - Added 60s execution timeout and process reaping to Typst CLI invocation.
- **FFmpeg Hardware Acceleration & Bitrate Support (`ffmpeg.rs`):**
  - Attached `-hwaccel` before input arguments for video conversions.
  - Implemented continuous range-based quality mapping for MP3 and OGG VBR bitrates (supporting standard bitrates like 320, 256, 128 kbps).
  - Added quote-aware command tokenization for custom FFmpeg commands.
- **Explorer Shell Extension Hardening (`lib.rs`):**
  - Disabled blank property sheet tab injection in Explorer Properties dialog.
  - Added boundary checking against `_idcmdlast` in `QueryContextMenu` to prevent menu ID overflow.
  - Hid Windows 11 context menu when no files are selected.
- **Hot-Path Optimization (`image.rs`):**
  - Cached `resvg` system font scanning via `LazyLock<Arc<Database>>`, eliminating seconds of font scanning latency on SVG conversions.
- **Packaging & Uninstaller Cleanup (`installer.nsi`, `.github/workflows/release.yml`):**
  - Added deletion of icons and `/REBOOTOK` handling for locked DLLs in the NSIS uninstaller, with `SHChangeNotify` cache refresh.
  - Bundled default XML and icons into the portable zip package.

---

## [0.9.4] - 2026-09-03

### 🐛 Fixed & Hardened
- **SVG Vector Scaling Accuracy (`image.rs`):**
  - Resolved double-scaling bug where SVG files were rasterized at target scale and then resized again during the general transform pass.
- **Robust Relative Path Handling (`path_helpers.rs`):**
  - Fixed `create_folders` failure on relative paths without directory components (`output.mp3`).
- **HTML Tag Stripper Markup Retention (`doc_convert.rs`):**
  - Fixed issue where unclosed angle brackets (`<`) caused subsequent document text to be truncated during tag stripping.
- **Resource Cleanup for GIF Conversions (`ffmpeg.rs`):**
  - Attached intermediate palette removal to final pass with an RAII cleanup guard to prevent orphaned palette files in `%TEMP%`.
- **HEIF Format Classification (`types.rs`):**
  - Added `.heif` extension to `FileCategory::Image` categorization for full compatibility with native image decoding.
- **Multi-Page Office Export Collision Avoidance (`office.rs`):**
  - Added automatic directory creation and unique collision-free naming across all pages during multi-page Office exports.
- **Reliable FFmpeg Package Fallback Mirror (`ffmpeg_download.rs`):**
  - Updated secondary download mirror to permanent 7.0.2 package URL matching the pinned SHA-256 checksum.
- **Shell Extension Binary Discovery (`file_converter_shell`):**
  - Added `%LOCALAPPDATA%\FileConverter` and `%ProgramFiles%\FileConverter` lookup fallbacks for reliable Explorer context menu launching.

---

## [0.9.3] - 2026-08-21

### 🚀 Enhanced & Refactored
- **Modernized HTTP & Extraction Pipeline (`ureq` 3 & `zip` 4):**
  - Upgraded HTTP engine to `ureq` v3.4.0 with centralized timeout configs and streaming body readers.
  - Upgraded archive decompression engine to `zip` v4.6.1 for robust multi-source FFmpeg bundle downloads.
- **Ultra-Fast Temporal Processing (`jiff`):**
  - Migrated date/time parsing and formatting from `chrono` to `jiff` (v0.2.35) for path template formatting and history logging.
- **Optimized Regex Footprint:**
  - Trimmed `regex` crate features to minimal required set (`["std", "perf", "unicode-case", "unicode-perl"]`), eliminating unneeded Unicode tables and reducing binary footprint.
- **Unified Workspace Dependencies (`[workspace.dependencies]`):**
  - Centralized dependency definitions and versions in root `Cargo.toml`, ensuring unified builds across `file_converter_core`, `file_converter_bin`, and `file_converter_shell`.

---

## [0.9.2] - 2026-08-20

### 🚀 Added & Enhanced
- **Zero-Allocation Inline Strings (`compact_str`):**
  - Replaced heap-allocated `String` with `compact_str::CompactString` across `PresetSetting` key-values and `preset.input_types`.
  - Eliminates over 750 heap allocations on application launch and Explorer shell context menu initialization.
- **Asynchronous GitHub Release Update Checker (`update_check.rs`):**
  - Added non-blocking background update checking on launch when `check_upgrade_at_startup` is enabled.
  - Displays dynamic in-app notification banner with direct download link when a newer version is released.
- **Preset Management Controls (New / Delete / Import / Export):**
  - Added dedicated `➕ New Preset`, `🗑️ Delete Preset`, `📥 Import XML`, and `📤 Export XML` action buttons in Slint Settings UI.

### 🐛 Fixed
- **Multi-Page Office to Image Export (`office.rs`):**
  - Resolved bug where converting multi-page Office documents (`.docx`, `.xlsx`, `.pptx`) to image formats (`.png`, `.jpg`, `.webp`, `.avif`) only rendered the first page.
  - Dynamically queries intermediate PDF page counts via `image::get_pdf_page_count` and generates sequence output paths (`(1)`, `(2)`, etc.) across all pages.

---

## [0.9.1] - 2026-08-20

### 🐛 Fixed & Enhanced
- **Direct Vector PDF Document Generation (`pdf-writer`):**
  - Resolved issue where EPUB, Markdown, and Typst conversions produced HTML files when targeting `.pdf`.
  - Integrated high-performance multi-page vector PDF generation via `pdf-writer` with text wrapping, typography formatting, and pagination.
- **Hardware Acceleration Filter Chain Fix:**
  - Resolved `HardwareAccelerationMode::Auto` to concrete hardware acceleration before building video transform arguments, fixing NVIDIA NVENC filter graph errors where CPU scaling filters collided with CUDA decoded frames.
- **Out-of-the-Box Shell Context Menu:**
  - Embedded `Settings.default.xml` directly into `file_converter_shell.dll` ensuring Explorer right-click context menu presets appear immediately on fresh installations prior to first GUI launch.
- **Work-Stealing Rayon Concurrency & Parallel PDF Optimization:**
  - Replaced custom `mpsc` thread pool in `ConversionScheduler::execute_all` with `rayon::ThreadPool` and `par_iter()` work-stealing execution.
  - Parallelized embedded image downscaling and multi-threaded DCT/Flate recompression across PDF stream objects in `pdf_compress.rs`.
- **Configurable OxiPNG Presets (`oxipng`):**
  - Updated `run_oxipng_compression` to parse and apply preset settings for `OxipngOptimizationLevel` (levels 1–6), `OxipngStrip` (`all`, `safe`, `none`), and `OxipngInterlace` (`Adam7`, `None`).
- **$O(1)$ AHashMap Preset Resolution (`ahash`):**
  - Added `Settings::build_preset_map` returning `AHashMap<&str, &ConversionPreset>` for fast lookups by name.
- **JPEG XL Output Encoding (`OutputType::Jxl`):**
  - Added `OutputType::Jxl` encoding pass in `ffmpeg.rs` using `libjxl` with video transformation parameters.
- **Trimmed `derive_more` Compilation Footprint:**
  - Reduced `derive_more` features to `["is_variant"]` in `file_converter_core/Cargo.toml`, cutting downstream compilation time.
- **Slint UI Interactive Cancel Controls & DND:**
  - Added visible "✕ Cancel" buttons per active conversion row in `ProgressWindow` wired directly to `cancel_job(id)`.
  - Wired dropzone click and file drop triggers in `SettingsWindow`.
  - Dynamically updated `overall_status_text` with real-time conversion progress and failure counts.
- **Unified Command-Line Parser (`clap`):**
  - Eliminated duplicate manual string parser loop in `main.rs`.
  - Added argument normalization for Windows-style slash flags (`/preset`, `/settings`, `/input-files`) into canonical `clap` CLI options.
- **FFmpeg Error Diagnostics & Asynchronous Stderr Draining:**
  - Retained rolling stderr log buffer in `run_ffmpeg_pass` to output actionable FFmpeg error messages upon non-zero exit codes.
  - Added background asynchronous reader threads for PowerShell COM pipes in `office.rs`, preventing process deadlocks on >64KB stderr buffers.
- **Multi-Source FFmpeg Downloader with Executable Verification:**
  - Added multiple mirror download sources with request timeouts.
  - Added validation of extracted binary size and `MZ` PE executable header integrity.
- **Streamlined COM Shell Registration & Registry Footprint Reduction:**
  - Refactored `DllRegisterServer` to target only the canonical `*` (all files) and `Directory` (folders) shell associations instead of redundant multi-root writes across `Drive`, `Background`, and `Folder`.
  - Removed duplicate static `shell\FileConverter\command` verb subkey, letting Windows 11 `ExplorerCommandHandler` and `IContextMenu` handle invocations cleanly via COM.
  - Replaced `KEY_ALL_ACCESS` with least-privilege `KEY_WRITE` flags and prioritized system-wide `HKLM\Software\Classes` when elevated with clean fallback to `HKCU\Software\Classes`.
- **GDI 32bpp BGRA Little-Endian Icon Color Fix:**
  - Introduced explicit `rgb(r, g, b)` bitwise helper for `CreateBitmap` Little-Endian memory layout ensuring correct amber, blue, emerald green, and red hues.
- **Dead Code Pruning & Category Multi-Match Testing:**
  - Removed dead/broken `run_epub_optimization` stub.
  - Implemented `as_str()` on `FileCategory` and verified multi-category compatibility for `OutputType::Gif` across `AnimatedImage`, `Image`, and `Video`.

---

## [0.9.0] - 2026-08-19

### 🚀 Added & Enhanced
- **Native JPEG XL (`.jxl`) Support (`jxl-oxide`):**
  - Integrated `jxl-oxide` for native pure-Rust decoding of JPEG XL images to all supported target formats.
- **Direct Image-to-PDF Bundling (`pdf-writer`):**
  - High-performance, pure-Rust PDF creation directly from images using `pdf-writer` with DCT/Flate streams.
- **EBU R128 Audio Loudness Normalization (`loudnorm`):**
  - Broadcast-standard audio normalization filter integration across all audio and video conversion passes in `ffmpeg.rs`.
- **Dynamic GPU Hardware Acceleration Auto-Detection:**
  - Added `HardwareAccelerationMode::Auto` with cached runtime probing for NVIDIA NVENC (`CUDA`), AMD (`AMF`), and Intel (`QSV`).
- **EPUB 3 Lossless Optimization (`ebook-rs`):**
  - Lossless markup minification, CSS cleanup, and asset optimization for EPUB eBook files.
- **Windows 11 Modern Context Menu (`ExplorerCommandHandler`):**
  - Registered `ExplorerCommandHandler` and application icon keys to show File Converter directly on Windows 11 top-level right-click menus.
- **SIMD & Zero-Allocation Optimizations:**
  - `memchr` SIMD tag and path delimiter parsing.
  - `smallvec` stack-allocated path token collection avoiding heap allocator churn.
  - `bytemuck` zero-copy byte slice casting for rendered page buffers.
- **Comprehensive Unit Testing Suite:**
  - Added 28 descriptive unit and integration tests across `file_converter_core`, `file_converter_bin`, and `file_converter_shell`.

---

## [0.8.1] - 2026-08-12

### ⚡ Performance & Optimization
- **Fat Link-Time Optimization (`lto = "fat"`):**
  - Enabled Fat LTO in release profile for cross-crate binary size minimization and inline optimization.
- **Poison-Free Lock Concurrency (`parking_lot`):**
  - Refactored `ConversionJob` and `ConversionScheduler` Mutex locks to poison-free `parking_lot::Mutex`, eliminating `.unwrap()` locking panics and lowering lock memory footprint to 1 byte.
- **SIMD Stream Search (`memchr`):**
  - Integrated `memchr` byte searching for SIMD-accelerated log parsing.
- **LazyLock Regex Statics:**
  - Converted FFMpeg duration and progress regex patterns into thread-safe `LazyLock<Regex>` statics to eliminate regex recompilation allocations in hot loops.
- **3x Faster Map Lookups (`ahash::AHashMap`):**
  - Replaced standard HashMap in settings preset resolution with `ahash::AHashMap`.
- **Enum Macro Derivations (`strum` & `derive_more`):**
  - Derived `Display`, `EnumString`, `AsRefStr`, and `IsVariant` on all core conversion enums.

---

## [0.8.0] - 2026-08-12

### ⚡ Added (Seamless Fast UX & Minimal Click Workflow)
- **Zero-Click Auto-Start on File Drop (`auto_start_on_file_drop`):**
  - Files dropped into the Settings GUI dropzone automatically initiate conversion without extra prompt clicks.
- **Instant Auto-Close Mode (`0.0s` exit delay):**
  - Option to instantly close the progress window upon batch conversion completion.
- **Clipboard Output Auto-Copy & Quick Copy Button:**
  - One-click `📋 Copy Output Paths` button on completion screens.
  - Option to auto-copy converted output file paths to the clipboard automatically.
- **Fast Action Bar:**
  - Direct `📁 Open Output Folder` and `📋 Copy Output Paths` actions in progress dialogs.

### 🛠️ Maintenance & Refactoring
- Updated all workspace dependencies via `cargo update` to latest compatible crates.
- Verified workspace codebase zero-warning compliance with `cargo clippy -- -D warnings`.
- Updated release installer definition script (`installer.nsi`) to `v0.8.1`.

---

## [0.7.0] - 2026-07-28

### 🚀 Initial Feature Parity Release
- 100% Rust workspace rewrite of FileConverter with pure-Rust image engine, FFMpeg integration, and Windows Shell extension DLL.
