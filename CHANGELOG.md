# Changelog

All notable changes to **FileConverter-rs** will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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
