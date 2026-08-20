<p align="center">
  <img src="icon.png" width="128" height="128" alt="File Converter Logo" />
</p>

# FileConverter-rs

[![Release](https://img.shields.io/github/v/release/SV-stark/FileConverter-rs?color=blue&style=flat-square)](https://github.com/SV-stark/FileConverter-rs/releases)
[![License: GPL v3](https://img.shields.io/badge/License-GPL_v3-blue.svg?style=flat-square)](LICENSE)
[![Platform](https://img.shields.io/badge/Platform-Windows_10_%2F_11_x64-brightgreen.svg?style=flat-square)](https://github.com/SV-stark/FileConverter-rs)
[![Build Status](https://img.shields.io/github/actions/workflow/status/SV-stark/FileConverter-rs/release.yml?style=flat-square)](https://github.com/SV-stark/FileConverter-rs/actions)

A **100% feature-parity rewrite** of the popular open-source Windows utility **[FileConverter](https://github.com/Tichau/FileConverter)** in **Rust**.

This project modernizes and accelerates the core conversion pipelines, Windows Explorer shell context menu integrations, and settings dashboard of the original C# FileConverter into a highly optimized, memory-safe, concurrent, and standalone 64-bit Rust workspace.

---

## 💡 Credits & Attribution

This project is a direct rewrite and port of **[FileConverter](https://github.com/Tichau/FileConverter)** developed by **[Tichau](https://github.com/Tichau)**. 

All credit for the original application design, default presets schema (`Settings.default.xml`), conversion template logic, and Windows Explorer cascading context menu concepts goes to Tichau and the original C# project contributors.

---

## ⚡ Performance & Parity Overview

| Feature / Subsystem | Original C# FileConverter | Rust Rewrite (`FileConverter-rs`) |
| :--- | :--- | :--- |
| **Settings Schema** | XML (`Settings.default.xml` / `user.xml`) | 100% XML schema parity via `quick-xml` & `serde` |
| **Explorer Context Menu** | SharpShell C# COM Extension | **Pure-Rust `windows` Crate COM DLL** (`cdylib`) with embedded default presets, type-safe `IShellExtInit`, `IContextMenu`, `IShellPropSheetExt`, & Windows 11 `ExplorerCommandHandler` |
| **Windows Explorer Integration** | Legacy "Show more options" only | Native COM `shellex` + **Direct Windows 11 Top-Level Context Menu** + **Property Sheet inspection tab** |
| **Settings Dashboard** | WPF Settings Window (`SettingsWindow.xaml`) | **Native Desktop GUI Window** (`Slint UI` Fluent Design) |
| **Conversion Progress** | WPF Progress Window (`ProgressDialog.xaml`) | **Native Desktop Progress Window** with per-job live progress, cancel controls, dropzone actions & dynamic status indicators |
| **Image Conversion & PNG Compression** | External ImageMagick CLI binaries | **Pure-Rust Engine** (`image.rs`) using zero-copy `memmap2`, SIMD `fast_image_resize`, `jxl-oxide` (JPEG XL), & `oxipng` multi-level lossless PNG compression |
| **Direct Vector PDF Generation** | ImageMagick / HTML rasterization | **Pure-Rust** `pdf-writer` creating vector-accurate, multi-page PDF documents directly from eBooks (EPUB, MOBI, AZW), Markdown, Typst, and images |
| **Parallel Scheduling & PDF Optimization** | Serial / ThreadPool | **Rayon Work-Stealing Pool** for concurrent job conversion and parallel embedded image stream recompression (`pdf_compress.rs`) |
| **HEIC/HEIF Support** | ImageMagick / libheif binaries | **Pure-Rust** `heic` decoder with memory-mapped byte buffer parsing |
| **PDF Page Rasterization** | Ghostscript / ImageMagick | **Pure-Rust** `hayro` engine rendering pages in parallel with `rayon` across all CPU cores |
| **Document & E-Book Conversion** | Pandoc / Calibre / Office | **Pure-Rust Engine** (`ebook-rs`, `pulldown-cmark`, `typst`) converting EPUB, MOBI, AZW, Markdown, and Typst to PDF, HTML, and Text |
| **Audio/Video Conversion** | FFMpeg CLI execution | Optimized FFMpeg CLI wrapper with **GPU Auto-Detection** (CUDA / AMF / QSV), chunked stderr streaming, **EBU R128 Audio Normalization** (`loudnorm`), & JPEG XL encoding |
| **Office Conversion** | Word / Excel / PowerPoint COM Interop | Background PowerShell COM automation with asynchronous stderr draining and 5-minute timeout protection |

---

## 📖 Usage Guide

### 1. Converting Files from Windows Explorer
1. Select one or multiple files in **Windows Explorer**.
2. **Right-click** the selection:
   - On **Windows 10**: Hover over the **`File Converter`** cascading context menu.
   - On **Windows 11**: Click **`File Converter`** directly on the main context menu (or expand *"Show more options"*).
3. Select your desired target format (e.g., *"To Mp3"*, *"To Png"*, *"To Pdf"*).
4. The **Native Progress Window** will pop up, displaying individual progress bars per file, cancel buttons (`✕ Cancel`), and real-time status summary.
5. Once conversion completes, output files are placed directly alongside source files (or formatted per your path template settings), and the window automatically closes.

### 2. Batch Operations & Large Selection Handling
* You can select **hundreds or thousands of files at once**. 
* If the command-line length exceeds Windows limits (8,000 characters), FileConverter automatically passes file lists via a temporary text manifest file to ensure seamless batch execution.

### 3. Configuring Conversion Presets & Dashboard Features
1. Open **FileConverter** from the **Windows Start Menu** or click **`Configure presets...`** at the bottom of the right-click menu.
2. The **Native Settings Window** opens:
   - **🔍 Preset Search & Category Badges**: Filter presets in real time and view format badges (**🎵 Audio**, **🎬 Video**, **🖼️ Image**, **📄 Document**).
   - **📋 Preset Duplication**: Clone existing presets with a single click (**`📋 Duplicate`**).
   - **✨ Live Output Path Template Preview**: Real-time interactive preview box showing exact output path transformations before saving.
   - **📂 Drag & Drop Dropzone**: Drop files directly into the settings window to convert without right-clicking in Explorer.
   - **📜 Conversion History Log**: View a full log of recent batch conversions, execution timestamps, output file locations, and status.
   - **📁 Post-Conversion Quick Actions**: Click **`📁 Open Output Folder`** or **`📋 Copy Output Paths`** directly from the progress dialog.
3. Click **`💾 Save Settings`** to persist changes immediately.

---

## 💻 CLI Automation

FileConverter includes a unified command-line interface (`file_converter_bin`) powered by `clap` supporting standard options and Windows slash-flags:

```bash
# List all configured conversion presets
file_converter_bin list-presets

# Convert files via a preset (CLI / Headless)
file_converter_bin convert -p "To Mp3" song1.flac song2.wav
file_converter_bin convert -p "To Png" --headless ./photos/*.bmp

# Convert files with Windows Explorer syntax
file_converter_bin /preset "To Png" file1.jpg file2.jpg
file_converter_bin --conversion-preset "To Pdf" document.docx

# Open GUI Settings Dashboard
file_converter_bin --settings
```

---

## 🛠️ Architecture & Core Modules

The repository is structured as a modular Cargo workspace containing three distinct sub-crates:

```
FileConverter-rs/
├── file_converter_core/    # Core conversion library, XML parser, & scheduler
│   ├── src/
│   │   ├── doc_convert.rs  # Vector PDF generation (pdf-writer), eBook & Markdown
│   │   ├── ffmpeg.rs       # Audio & Video FFMpeg command builder, GPU auto-detect & pass runner
│   │   ├── ffmpeg_download.rs # Multi-mirror FFmpeg downloader with PE header verification
│   │   ├── image.rs        # Pure-Rust Image & PDF engine, oxipng multi-level compression
│   │   ├── office.rs       # Word, Excel, PowerPoint COM automation with async stderr drain
│   │   ├── pdf_compress.rs # Parallel PDF image stream downscaling (Rayon)
│   │   ├── scheduler.rs    # Rayon work-stealing threadpool & job coordinator
│   │   ├── settings.rs     # Preset parser, ahash O(1) map index & XML serializer
│   │   ├── path_helpers.rs # Output path template engine & unique filename generator
│   │   └── types.rs        # Strongly-typed FileCategory, OutputType, PostAction enums
├── file_converter_shell/   # Windows Shell Extension COM DLL
│   └── src/
│       └── lib.rs          # IContextMenu, IShellExtInit, IShellPropSheetExt, & DllRegisterServer
├── file_converter_bin/     # Native Desktop GUI & CLI Application
│   ├── src/
│   │   └── main.rs         # Slint Settings Dashboard & Progress Dialog
│   └── ui/
│       └── appwindow.slint # Slint UI Declarative Fluent interface definitions
├── Settings.default.xml    # 100% original C# conversion presets XML
└── installer.nsi           # 64-bit NSIS setup installer script
```

---

## ⚡ High-Performance Architecture Stack
* **Work-Stealing Concurrency (`rayon`)**: Thread pool scheduling for batch file conversions and parallel PDF stream downscaling.
* **Vector PDF Generation (`pdf-writer`)**: Direct generation of multi-page vector PDF documents from plain text, Markdown, Typst, and images.
* **Lossless PNG Optimization (`oxipng`)**: Configurable multi-level compression, metadata stripping, and interlace controls.
* **$O(1)$ Hash Map Lookups (`ahash`)**: `AHashMap` preset indexing eliminating string scanning bottlenecks.
* **Poison-Free Mutexes (`parking_lot`)**: Fast, lightweight synchronization primitives.
* **Zero-Copy File I/O (`memmap2`)**: Memory-mapped page buffers minimizing memory footprint.
* **SIMD Rescaling (`fast_image_resize`)**: Vectorized image resampling supporting AVX2, SSE4.1, and NEON.
* **Resilient Multi-Source Downloader (`ureq` + `zip`)**: Automatic FFmpeg extraction with PE header validation and fallback mirrors.

---

## 📦 Installation & Integration

### Download Official Release
Download the latest 64-bit installer or portable zip package from [GitHub Releases](https://github.com/SV-stark/FileConverter-rs/releases):
* **`FileConverter_Setup.exe`**: Automatic installer. Registers 64-bit COM shell extensions natively into Windows Explorer and adds shortcuts to the Start Menu.
* **`FileConverter_Portable.zip`**: Standalone portable archive containing all binaries.

### Manual Shell Extension Registration
To manually register or unregister the Windows Explorer right-click context menu extension:

```powershell
# Register context menu DLL (Run as Administrator)
regsvr32.exe /s file_converter_shell.dll

# Unregister context menu DLL
regsvr32.exe /u /s file_converter_shell.dll
```

Or open `file_converter_bin.exe` and click **"⚙️ Register Shell Extension"**.

---

## 🔨 Building from Source

### Prerequisites
* [Rust](https://www.rust-lang.org/) (1.80+ recommended)
* Windows 10/11 64-bit
* FFMpeg binary (`ffmpeg.exe` in `PATH` or current directory for media conversions)

### Compile Workspace

```powershell
# Check workspace code & linting
cargo check --workspace
cargo clippy -- -D warnings

# Build optimized release binaries
cargo build --release --workspace
```

The output artifacts will be placed in `target/release/`:
* `file_converter_bin.exe`
* `file_converter_shell.dll`

### Build Setup Installer (NSIS)

```powershell
# Requires NSIS installed
makensis /V4 installer.nsi
```

---

## 📜 License

This project is licensed under the **GNU General Public License v3.0 (GPL-3.0)**. See the [LICENSE](LICENSE) file for complete details.
