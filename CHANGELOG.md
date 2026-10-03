# Changelog

All notable changes to **FileConverter-rs** will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [Unreleased]

### Fixed - Explorer context menu (shell extension)

Found by a line-by-line audit of `file_converter_shell`. The COM extension is
loaded *inside* `explorer.exe`, which made several bugs far more severe than they
would be in a standalone process.

- **Concurrent right-clicks converted the wrong files (`lib.rs`).**
  The large-selection input list was written to
  `file-converter-input-list-<pid>.txt`. Inside Explorer the PID is always
  `explorer.exe`'s, so every invocation used the *same* path and `File::create`
  truncated the previous list — two right-clicks within seconds of each other
  silently converted the first selection with the second preset. The file is now
  uniquely named and created with `create_new`, so it is never truncated and a
  pre-planted file is never followed.
- **Paths with leading/trailing spaces were corrupted (`main.rs`).** The
  newline-delimited input list was read back with `line.trim()`, which also ate
  spaces that are legal in Windows paths (`C:\My Files\ a.png`) and converted a
  non-existent file. Only the line terminator is stripped now.
- **Out-of-range menu command IDs (`lib.rs`).** The guard
  `if _idcmdlast >= idcmdfirst && (idcmdfirst + needed > _idcmdlast)` short-circuited
  to `false` when the shell had reserved no ID range, so the fall-through path
  emitted IDs belonging to the next handler. The guard now also uses checked
  addition, and the reported item count is the number of items actually inserted.
- **USER menu handle leak (`lib.rs`).** The popup menu handle leaked on the
  failed-insertion path, on every context-menu display inside explorer.exe. It is
  now released by a guard unless ownership transfers to the menu.
- **Registration always reported success (`lib.rs`).** Every
  `create_subkey`/`set_value` result was discarded and `DllRegisterServer`
  returned `S_OK`, so `regsvr32` reported success even when nothing was written.
  Failures are now logged and reported as `E_FAIL`, and a stale `Directory`
  association that offered all 79 presets for a folder selection was dropped.
- **A relative path could be written to the registry (`lib.rs`).** If the module
  path could not be determined, the association loop wrote
  `"file_converter_bin.exe" "%1"`, which Explorer resolves against
  `%WINDIR%`. Registration now aborts instead of writing a relative command.
- **The modern verb silently did nothing (`lib.rs`).** `IExplorerCommand::Invoke`
  discarded the spawn result and returned `S_OK`, and `GetState` reported
  `ECS_ENABLED` without checking that the binary exists. Both now surface the
  real failure and hide the verb when it cannot run.
- **Empty paths made every preset appear compatible (`lib.rs`).** The
  `DragQueryFileW` fill result was unchecked, so a failed query pushed an empty
  path; because extensionless paths are treated as compatible, one such entry
  made all 79 presets show for an unrelated selection. Fill failures are now
  skipped, `size + 1` no longer overflows, and non-file items (folders) are
  filtered out.
- **Allocations inside `DllMain` (`lib.rs`).** `dbg_log!` (which formats) ran under
  the loader lock, where a panic would abort `explorer.exe`. Diagnostics were
  removed from `DllMain` and `DisableThreadLibraryCalls` is now called.
- **Miscellaneous (`lib.rs`):** `LockServer(FALSE)` no longer underflows the lock
  count to `u32::MAX` (which pinned `DllCanUnloadNow` at `S_FALSE` forever);
  `CreateInstance` now nulls its out-parameter on failure as COM requires;
  `GetModuleFileNameW` results are truncated at the first NUL (a truncated path
  previously embedded a U+0000 and failed every `exists()` check).

### Fixed - core

- **The Cancel button did nothing until the job finished (`scheduler.rs`).**
  `cancel` only flipped a status flag; no engine ever polled it, so aborting a
  long video conversion had no effect until FFmpeg exited. Cancellation is now
  cooperative — polled between FFmpeg passes (so multi-pass jobs such as video to
  GIF abort before the second pass) — and a job cancelled mid-run is reported as
  `Canceled` rather than as a failure with a confusing message.
- **A panic in the progress timer (`main.rs`):** `slint::quit_event_loop().unwrap()`
  ran inside a Slint timer callback; it no longer panics.

### Fixed - CI reliability
- **A floating `stable` toolchain broke an unchanged tree** (`.github/workflows/`).
  `dtolnay/rust-toolchain@stable` combined with `RUSTFLAGS: -D warnings` means any
  new release can fail the build purely through newly added lints. Rust 1.99
  renamed `Atomic::fetch_update` to `try_update`, so the previous commit's
  `LockServer` fix became a deprecation warning and therefore a CI failure with no
  source change at all. Both workflows now pin `1.99.0`, and `LockServer` uses
  `compare_exchange` so it depends on neither spelling. Verified by running the
  exact CI sequence (fmt, clippy `-D warnings`, build, tests) on 1.99.0.

### Notes
- `cargo fmt` was not applied before the v0.10.0 tag, so the CI workflow failed on
  `main` (formatting only - clippy, the build and all tests passed). Fixed in the
  commit immediately after the tag.

---

## [0.10.0] - 2026-10-03

### Fixed - conversions other than PDF

A full preset x input-type matrix was used to drive these fixes. Every case now
passes except `To Ogv`, which requires the `libtheora` encoder (absent from
minimal FFmpeg builds; the app now reports that explicitly instead of failing
with `Error selecting an encoder`).

- **Text/markup documents were routed to the image engine** (`scheduler.rs`, `doc_convert.rs`):
  `.txt`, `.html`, `.csv`, `.json`, `.log` and `.rtf` fell through to the raster
  pipeline and failed with `Failed to load image from memory map ... Format(Unknown)`
  for every output type. They now have a dedicated `JobEngine::TextDocument`
  engine that produces PDF, plain text, HTML, or a rasterised A4 document page.
- **Document engines ignored the requested output type** (`doc_convert.rs`):
  eBook / Markdown / Typst conversions always wrote HTML, so `To Png` on a `.md`
  file silently produced an HTML document named `....png`. All text-producing
  engines now share one `write_document_output` writer that honours
  `Pdf` / `Txt` / `Html` / raster outputs (new `Txt` and `Html` output types).
- **GIF conversion from still images** (`scheduler.rs`, `image.rs`, `ffmpeg.rs`):
  `scale` + `fps` + `palettegen` produced an *empty* palette file for single-frame
  sources, so the second pass failed with
  `Error opening input file ... fc_palette_*.png: No such file or directory`.
  Still images (and still WebP) are now encoded natively with `GifEncoder`;
  FFmpeg's palette recipe is reserved for genuinely animated input, and its
  palette pass no longer applies `fps`. The palette file is also cleaned up when
  a pass chain aborts.
- **WebM / OGV / AVI output rejected odd-sized or non-yuv420p sources** (`ffmpeg.rs`):
  `libvpx-vp9` failed with `Error while opening encoder` on GIF input. Video
  outputs now always emit an even-dimension `scale` filter plus `-pix_fmt yuv420p`.
- **ICO conversion** (`image.rs`, `scheduler.rs`): output was forced into a 256x256
  square (destroying the aspect ratio) and depended on FFmpeg's ICO muxer, which
  rejects pages larger than 256 px (`Could not write header`). ICO is now written
  natively as a multi-resolution container (16/24/32/48/64/128/256).
- **PDF input ignored scale / rotation / clamp settings** (`image.rs`):
  rendered pages skipped the transform pipeline, so `To Ico` and `Scale x%` on a
  PDF produced full-page images. PDF pages now run through the same
  `apply_image_transforms` stage as still images.
- **`preset.input_types` was never enforced** (`scheduler.rs`, `types.rs`):
  applying `Compress Png (lossless)` to a JPEG reached OxiPNG and failed with
  `Invalid PNG header detected`. `ConversionJob::validate` now rejects
  unsupported inputs with an actionable message, and the UI filter and the
  scheduler share one `is_preset_applicable_to_file` implementation.
- **Preparation failures were swallowed** (`main.rs`, `scheduler.rs`):
  `create_conversion_jobs` logged and dropped the error, so the job later failed
  with `No output path specified for job`. The reason is now stored on the job and
  reported verbatim.
- **Success was reported for outputs that do not exist** (`scheduler.rs`):
  `Done` is now only reported when every declared output file is present.
- **FFmpeg timeout could never fire** (`ffmpeg.rs`): the 1-hour watchdog was
  checked around a blocking `stderr.read()`. Progress parsing now runs on a reader
  thread with a channel timeout, so a hung FFmpeg is actually killed.
- **GIF palette temp file was deleted before use** (`ffmpeg.rs`):
  `TempPath::into_temp_path().to_path_buf()` dropped the guard immediately,
  deleting the palette path. A collision-free temp path is now generated without
  creating the file.
- **Settings window never persisted anything** (`main.rs`):
  `on_save_settings` only wrote `duration_between_end_of_conversions_and_application_exit`,
  silently discarding every preset edit and preference toggle. All mutable
  preferences are now read back from the UI and saved.
- **Preset selection used filtered row indices** (`main.rs`): selecting,
  duplicating or deleting a preset while a search filter was active operated on
  the wrong preset (and reset the filter). Rows are now mapped back through
  `PresetData.original_index`.
- **Missing Office / FFmpeg encoders produced opaque errors** (`office.rs`, `ffmpeg.rs`):
  both are now probed up-front and reported with actionable guidance.

### Changed
- **The clipboard was never actually written** (`scheduler.rs`):
  `clipboard_win::raw::set_file_list` uses its `NoClear` variant, which does *not*
  open the clipboard, so every "copy results to clipboard" call failed with
  `ERROR_CLIPBOARD_NOT_OPEN (1418)` and was silently swallowed. The clipboard is
  now opened, emptied, populated and closed explicitly.
- **Paged outputs reported the wrong total** (`scheduler.rs`, `office.rs`):
  `(n:c)` was filled with the number of *input files*, so an 8-page PDF produced
  `file 1 of 1.png` … `file 8 of 1.png`. It now reports the number of files
  produced for the current input.
- **Newly shipped presets now reach existing installations** (`main.rs`):
  presets present in the shipped defaults but missing from the user's
  `Settings.user.xml` are merged in on startup (additive only - user edits are
  never overwritten). Note that *corrected* presets that already exist under the
  same name keep the user's stored definition; use "Import Presets" or delete
  `Settings.user.xml` to pick those up.
- **Image to PDF page size** (`image.rs`): pages were one PDF point per pixel
  (a 4000 px photo became a 55 inch page). Bitmaps are now treated as 96 DPI.
- **`ImageClampSizePowerOf2`** (`image.rs`): forced a square; each axis is now
  rounded down to a power of two independently, preserving the aspect ratio.
- **`ImageScale` / `ImageRotation` parsing** (`image.rs`, `ffmpeg.rs`): accepts
  comma decimals, rejects non-finite or non-positive values, and rotation is
  normalised modulo 360 degrees.
- **New default presets** (`Settings.default.xml`): `To Text (from EPUB/Markdown)`
  now has `OutputType="Txt"` (it previously claimed `Pdf`); added
  `To Html (from EPUB/Markdown/Text)`, `To Png (from Text/Markdown/EPUB)` and
  `To Jpg (from Text/Markdown/EPUB)`; `To Pdf` and
  `To Pdf (from Markdown/Typst/EPUB)` now accept text/markup inputs.
- **Raw camera formats report a clear error** (`image.rs`): `arw`, `cr2`, `nef`,
  `psd`, `xcf`, ... return an actionable message instead of
  `Failed to decode image data`.
- **PDF pages are unpremultiplied** (`image.rs`): hayro/vello emits premultiplied
  alpha, which produced fringes on transparent PDF pages.
- **`OutputType::Jxl` keeps using FFmpeg's `libjxl` encoder** (`scheduler.rs`):
  `jxl-oxide` is decode-only, so routing JXL output to the image engine would have
  produced an unsupported-encoder error.
- **Duplicate `Settings.default.xml` files removed** (`file_converter_bin/`,
  `file_converter_shell/`): both crates embedded a stale crate-local copy
  (76 presets) while the packaged file at the repository root had drifted. The
  crates now `include_str!("../../Settings.default.xml")`, so the embedded
  presets always match what the installer and the release archive ship.
- **`--version` and `--help` no longer launch the GUI** (`main.rs`): clap's
  help/version/usage errors were treated as "invalid arguments", so asking for
  the version silently opened the settings window. They now print and exit, and
  genuine usage errors are reported on stderr with a hint.
- **`--version` is derived from the crate version** (`main.rs`): the flag was
  hardcoded to `0.9.4` and had drifted. It now uses `env!("CARGO_PKG_VERSION")`.

### Safety
- **`file_converter_core` is now free of `unsafe`.** All three
  `unsafe { Mmap::map(..) }` blocks in `image.rs` and `pdf_compress.rs` were
  replaced with owned `std::fs::read` buffers, and the `memmap2` dependency was
  dropped. The conversion engines are pure safe Rust.
- **`file_converter_bin`:** the hand-rolled `extern "system"` declarations for
  `ShellExecuteW` and `MessageBeep` were replaced with `windows` crate bindings.
  The remaining `unsafe` is limited to unavoidable Win32/COM interop (file dialog,
  window lookup, `WM_DROPFILES` subclassing, `ShellExecuteW`).

---

## [0.9.7] - 2026-09-12

### ⚡ Performance & Memory Optimization
- **Zero-Copy SIMD Image Resizing (`image.rs`):**
  - Eliminated full-image bitmap cloning (`buf.as_raw().clone()`) by passing borrowed slice references through `fast_image_resize::images::ImageRef`.
  - Used `dst_image.into_vec()` to transfer ownership of output image buffers directly without copying.
- **Header-Only Download Verification (`ffmpeg_download.rs`):**
  - Eliminated reading the entire ~100MB `ffmpeg.exe` binary into memory during integrity validation, replacing it with filesystem metadata size check and a 2-byte stream read of `b"MZ"`.
  - Replaced heap-allocated 64KB vector with a fixed stack buffer for SHA-256 computation.
- **Fast Path Template Formatting (`path_helpers.rs`):**
  - Guarded all token replacements (`(path)`, `(p)`, `(F)`, `(O)`, `(I)`, `(d:)`, `(n:i)`, etc.), eliminating redundant Windows Registry queries and uppercase string allocations unless the respective tokens are present in the template.
- **Rayon Global Thread Pool Reuse (`scheduler.rs`):**
  - Reused Rayon's global thread pool whenever job concurrency matches the system thread pool, avoiding thread pool construction and teardown overhead per conversion batch.
- **Zero-Allocation Version Comparison & Document Streaming (`update_check.rs`, `doc_convert.rs`):**
  - Replaced heap-allocated `Vec<u32>` version vectors with an iterator-based comparison pipeline.
  - Eliminated intermediate vector allocations during plain text wrapping and page chunking.

### 🛡️ Safety & Reliability
- **Rust 2024 Safety Invariants & Annotations:**
  - Added comprehensive `// SAFETY:` rationale above all `Mmap::map` memory-mapped I/O blocks across PDF and image rasterization (`image.rs`, `pdf_compress.rs`).
  - Added safety invariant annotations for Win32 API calls (`ShellExecuteW`, `MessageBeep`, `CreateBitmap`, `DragQueryFileW`).
  - Encapsulated GDI category icon generation within a safe function with internal safety comments.
- **Windows COM RAII Cleanup (`main.rs`):**
  - Wrapped `CoInitializeEx` with `scopeguard::guard` to guarantee `CoUninitialize()` is invoked on all return paths in file dialogs.
  - Converted runtime wide-string allocations to compile-time `windows::core::w!` literals.
- **Standardized Error Casing (`error.rs`):**
  - Updated all `FileConverterError` messages to lowercase without trailing punctuation according to standard Rust conventions.

---

## [0.9.6] - 2026-09-08

### 🎨 GUI & Architecture
- **Slint GUI Overhaul & Theme Support:**
  - Redesigned user interface with Fluent Design styling, dark/light theme switching, and live format studio controls.
  - Embedded local `Settings.default.xml` and refined package metadata for crate distribution.

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
