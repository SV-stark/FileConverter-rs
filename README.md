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
| **Explorer Context Menu** | SharpShell C# COM Extension | **Pure-Rust COM DLL** (`cdylib`) registered in native 64-bit `HKCR` |
| **Windows 11 Menu** | Legacy "Show more options" only | Native COM `shellex` + **Direct Windows 11 Shell Verb** on main menu |
| **Settings Dashboard** | WPF Settings Window (`SettingsWindow.xaml`) | **Native Desktop GUI Window** (`eframe` / `egui`) |
| **Conversion Progress** | WPF Progress Window (`ProgressDialog.xaml`) | **Native Desktop Progress Window** (`ProgressApp`) with per-job bars & countdown |
| **Image Conversion** | External ImageMagick CLI binaries | **Pure-Rust Engine** (`image.rs`) using zero-copy `memmap2` & SIMD `fast_image_resize` |
| **HEIC/HEIF Support** | ImageMagick / libheif binaries | **Pure-Rust** `heic` decoder with memory-mapped byte buffer parsing |
| **PDF Page Rasterization** | Ghostscript / ImageMagick | **Pure-Rust** `hayro` engine rendering pages in parallel with `rayon` across all CPU cores |
| **Audio/Video Conversion** | FFMpeg CLI execution | Optimized FFMpeg CLI wrapper supporting Hardware Acceleration (CUDA/AMF) |
| **Office Conversion** | Word / Excel / PowerPoint COM Interop | Background PowerShell COM automation with intermediate PDF fallback |
| **CD Audio Extraction** | Native Win32 Drive IOCTLs | Safe Rust Raw Drive Sector Reader (`cda.rs`) |

---

## 🛠️ Architecture & Core Modules

The repository is structured as a modular Cargo workspace containing three distinct sub-crates:

```
FileConverter-rs/
├── file_converter_core/    # Core conversion library, XML parser, & scheduler
│   ├── src/
│   │   ├── image.rs        # Pure-Rust Image & PDF engine (replaces ImageMagick)
│   │   ├── ffmpeg.rs       # Audio & Video FFMpeg command builder & pass runner
│   │   ├── office.rs       # Word, Excel, PowerPoint COM automation
│   │   ├── cda.rs          # Audio CD track extraction via raw sector reading
│   │   ├── scheduler.rs    # Thread pool job queue & timestamp synchronization
│   │   ├── settings.rs     # Preset parser & XML serializer
│   │   ├── path_helpers.rs # Output file template engine ((p)\(f) resolution)
│   │   └── types.rs        # Enums for OutputType, PostAction, HW Acceleration
├── file_converter_shell/   # Windows Shell Extension COM DLL
│   └── src/
│       └── lib.rs          # IContextMenu / IShellExtInit implementation
├── file_converter_bin/     # Native Desktop GUI & CLI Application
│   └── src/
│       └── main.rs         # eframe Settings Dashboard & Progress Dialog
├── Settings.default.xml    # 100% original C# conversion presets XML
└── installer.nsi           # 64-bit NSIS setup installer script
```

---

## 🚀 Conversion Engines Detail

### 1. Pure-Rust Image & PDF Engine (`image.rs`)
* **Zero External Dependencies**: Operates completely without needing ImageMagick or Ghostscript installed on the host machine.
* **SIMD Rescaling**: Employs `fast_image_resize` using CPU SIMD vector instructions (AVX2/NEON/SSE4.1).
* **Zero-Copy File I/O**: Memory-maps input images and PDF files via `memmap2` to minimize memory allocation overhead.
* **Parallel PDF Rendering**: Uses `hayro` to parse PDF page structures and rasterizes multiple PDF pages concurrently across all CPU threads via `rayon`.
* **HEIC / HEIF Picture Support**: Decodes camera picture files natively using the `heic` crate.

### 2. Audio & Video Engine (`ffmpeg.rs`)
* Converts any media stream to `MP3`, `AAC`, `FLAC`, `OGG`, `WAV`, `MP4`, `MKV`, `WEBM`, `AVI`, `OGV`, `GIF`, or `ICO`.
* Full support for hardware-accelerated video encoding modes:
  * **NVIDIA CUDA** (`h264_nvenc`, `hevc_nvenc`)
  * **AMD AMF** (`h264_amf`, `hevc_amf`)
* Automatically calculates two-pass encoding for target file sizes when configured in preset settings.

### 3. Native Desktop GUI & Progress Windows (`main.rs`)
* Built using `eframe` (`egui`) for hardware-accelerated, instantaneous UI rendering.
* **Settings Window**: Complete preset list management (Add, Delete, Reorder Up/Down), live sample path output previews, max concurrency drag controls, and one-click shell extension registration.
* **Progress Window**: Triggered automatically when converting files from Windows Explorer right-click menus. Displays real-time per-file progress bars, overall status, and an auto-closing timer countdown.

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
