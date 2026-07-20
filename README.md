# FileConverter-rs

A 100% feature-parity rewrite of the popular Windows utility **[FileConverter](https://github.com/Tichau/FileConverter)** in **Rust**.

This project modernizes and adapts the core conversion architectures, shell context-menu integrations, and dashboard configuration of the original C# FileConverter into a highly optimized, concurrent, memory-safe, and dependency-light Rust workspace.

---

## Credits

This project is a direct rewrite and port of **[FileConverter](https://github.com/Tichau/FileConverter)** developed by **[Tichau](https://github.com/Tichau)**. All credit for the original design, settings schema, conversion presets, and shell cascading menu concepts goes to the original C# project creators and contributors.

---

## Features & Parity Status (100% Complete)

- **Conversion Engines**:
  - **FFMpeg CLI wrapper**: Full support for video and audio conversions (AAC, AVI, FLAC, ICO, GIF, MP3, MKV, MP4, OGG, OGV, PNG, WAV, WEBM).
  - **Pure-Rust Image Processor**: Fast image transformations (rotations, scaling, maximum size limits, and power-of-two clamps) utilizing the `image` crate with SIMD-accelerated resizing via `fast_image_resize` and zero-copy Memory-Mapped file I/O (`memmap2`).
  - **Pure-Rust HEIC/HEIF Decoder**: Built-in support for decoding `.heic` and `.heif` camera pictures natively using `heic`.
  - **Pure-Rust PDF Rasterizer**: High-performance, multi-threaded PDF-to-image converter using `hayro` (rasterizing PDF pages concurrently across all CPU cores).
  - **COM Office Automation**: Excel, Word, and PowerPoint exports to PDF/Images via silent, background PowerShell COM automation.
  - **CD Audio Extractor**: Direct sector reading from logical CD-ROM devices using native Windows IOCTLs.
- **Concurrent Job Scheduler**:
  - A bounded, worker thread pool running conversions concurrently up to custom limits.
  - CD-ROM drive locking (guarantees single-reader thread safety during CDA extraction).
  - Preserves input file access/modification timestamps on generated outputs.
  - Automatically copies conversion output files list to the Windows Clipboard (`CF_HDROP`).
  - Executes post-conversion rules (Delete Inputs or Move to Archive folder).
- **Windows Context-Menu Extension**:
  - Exposes COM context menu handlers (`IShellExtInit` / `IContextMenu`) in a native DLL (`cdylib`).
  - Automatically registers/unregisters with the system (`regsvr32`).
  - Dynamically builds cascading context submenus depending on compatible file formats.
- **Settings Dashboard GUI & CLI**:
  - Hosts a gorgeous, glassmorphic dark-theme settings web dashboard served locally.
  - Live cursor-rewriting progress reporting in the terminal for CLI batch conversions.

---

## Project Structure

The project is structured as a Cargo workspace containing three crates:

- **`file_converter_core`**: The main library holding data models, XML configuration parser, conversions registry, and scheduler.
- **`file_converter_shell`**: The Windows COM Context Menu shell extension DLL.
- **`file_converter_bin`**: The CLI runner and GUI settings dashboard web server.

---

## Building

To build the project in release mode:

```powershell
cargo build --release --workspace
```

The output files will be compiled to `target/release/`:
* `file_converter_bin.exe` (main CLI runner & web GUI launcher)
* `file_converter_shell.dll` (context menu DLL, register with `regsvr32.exe file_converter_shell.dll`)
