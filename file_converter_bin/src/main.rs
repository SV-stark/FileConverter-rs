#![allow(clippy::collapsible_if)]
#![windows_subsystem = "windows"]

use std::env;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use file_converter_core::scheduler::{ConversionJob, ConversionScheduler, JobStatus};
use file_converter_core::settings::Settings;
use file_converter_core::types::{HardwareAccelerationMode, OutputType};

slint::include_modules!();

fn get_settings_paths() -> (PathBuf, PathBuf) {
    let mut exe_dir = env::current_exe().unwrap_or_default();
    exe_dir.pop();

    let default_xml = exe_dir.join("Settings.default.xml");

    let local_app_data = env::var("LOCALAPPDATA").unwrap_or_default();
    let user_xml = Path::new(&local_app_data)
        .join("FileConverter")
        .join("Settings.user.xml");

    (default_xml, user_xml)
}

const DEFAULT_SETTINGS_XML: &str = include_str!("../../Settings.default.xml");

fn initialize_user_settings_if_needed() -> Result<Settings, String> {
    let (default_xml, user_xml) = get_settings_paths();

    if !user_xml.exists() {
        if let Some(parent) = user_xml.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        if default_xml.exists() {
            let _ = std::fs::copy(&default_xml, &user_xml);
        } else {
            let _ = std::fs::write(&user_xml, DEFAULT_SETTINGS_XML);
        }
    }

    Settings::load_from_file(&user_xml).map_err(|e| format!("Failed to load settings: {:?}", e))
}

fn register_shell_extension_dll() -> String {
    let mut exe_dir = env::current_exe().unwrap_or_default();
    exe_dir.pop();
    let dll_path = exe_dir.join("file_converter_shell.dll");

    if !dll_path.exists() {
        return format!("Shell DLL not found at {:?}", dll_path);
    }

    #[cfg(target_os = "windows")]
    unsafe {
        unsafe extern "system" {
            fn ShellExecuteW(
                hwnd: *mut std::ffi::c_void,
                lpOperation: *const u16,
                lpFile: *const u16,
                lpParameters: *const u16,
                lpDirectory: *const u16,
                nShowCmd: i32,
            ) -> *mut std::ffi::c_void;
        }

        let verb: Vec<u16> = "runas\0".encode_utf16().collect();
        let file: Vec<u16> = "regsvr32.exe\0".encode_utf16().collect();
        let params: Vec<u16> = format!("/s \"{}\"\0", dll_path.to_string_lossy())
            .encode_utf16()
            .collect();

        let res = ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            file.as_ptr(),
            params.as_ptr(),
            std::ptr::null(),
            1,
        );

        if (res as usize) > 32 {
            "Shell extension context menu registered successfully with administrator privileges!"
                .to_string()
        } else {
            format!(
                "Registration request failed or was canceled (Code: {}).",
                res as usize
            )
        }
    }

    #[cfg(not(target_os = "windows"))]
    "Shell extension registration is only supported on Windows.".to_string()
}

fn unregister_shell_extension_dll() -> String {
    let mut exe_dir = env::current_exe().unwrap_or_default();
    exe_dir.pop();
    let dll_path = exe_dir.join("file_converter_shell.dll");

    if !dll_path.exists() {
        return format!("Shell DLL not found at {:?}", dll_path);
    }

    #[cfg(target_os = "windows")]
    unsafe {
        unsafe extern "system" {
            fn ShellExecuteW(
                hwnd: *mut std::ffi::c_void,
                lpOperation: *const u16,
                lpFile: *const u16,
                lpParameters: *const u16,
                lpDirectory: *const u16,
                nShowCmd: i32,
            ) -> *mut std::ffi::c_void;
        }

        let verb: Vec<u16> = "runas\0".encode_utf16().collect();
        let file: Vec<u16> = "regsvr32.exe\0".encode_utf16().collect();
        let params: Vec<u16> = format!("/u /s \"{}\"\0", dll_path.to_string_lossy())
            .encode_utf16()
            .collect();

        let res = ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            file.as_ptr(),
            params.as_ptr(),
            std::ptr::null(),
            1,
        );

        if (res as usize) > 32 {
            "Shell extension context menu unregistered successfully!".to_string()
        } else {
            format!(
                "Unregistration request failed or was canceled (Code: {}).",
                res as usize
            )
        }
    }

    #[cfg(not(target_os = "windows"))]
    "Shell extension unregistration is only supported on Windows.".to_string()
}

fn play_completion_sound() {
    #[cfg(target_os = "windows")]
    unsafe {
        unsafe extern "system" {
            fn MessageBeep(uType: u32) -> i32;
        }
        let _ = MessageBeep(0x00000040);
    }
}

#[derive(Debug, serde::Serialize, serde::Deserialize, Clone)]
struct HistoryRecord {
    timestamp: String,
    preset_name: String,
    input_path: String,
    output_path: String,
    status: String,
}

fn get_history_path() -> PathBuf {
    let local_app_data = env::var("LOCALAPPDATA").unwrap_or_default();
    Path::new(&local_app_data)
        .join("FileConverter")
        .join("history.json")
}

fn load_history() -> Vec<HistoryRecord> {
    let p = get_history_path();
    if p.exists() {
        if let Ok(content) = std::fs::read_to_string(p) {
            if let Ok(list) = serde_json::from_str::<Vec<HistoryRecord>>(&content) {
                return list;
            }
        }
    }
    Vec::new()
}

fn save_history(history: &[HistoryRecord]) {
    let p = get_history_path();
    if let Some(parent) = p.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(history) {
        let _ = std::fs::write(p, json);
    }
}

fn add_history_record(preset_name: &str, input_path: &str, output_path: &str, status: &str) {
    let mut history = load_history();
    let now = jiff::Zoned::now().strftime("%Y-%m-%d %H:%M:%S").to_string();
    history.insert(
        0,
        HistoryRecord {
            timestamp: now,
            preset_name: preset_name.to_string(),
            input_path: input_path.to_string(),
            output_path: output_path.to_string(),
            status: status.to_string(),
        },
    );
    history.truncate(100);
    save_history(&history);
}

fn get_category_badge(output_type: OutputType) -> &'static str {
    match output_type {
        OutputType::Aac
        | OutputType::Flac
        | OutputType::Mp3
        | OutputType::Ogg
        | OutputType::Wav => "🎵 Audio",
        OutputType::Avi
        | OutputType::Mkv
        | OutputType::Mp4
        | OutputType::Ogv
        | OutputType::Webm => "🎬 Video",
        OutputType::Avif
        | OutputType::Ico
        | OutputType::Jpg
        | OutputType::Png
        | OutputType::Webp
        | OutputType::Gif => "🖼️ Image",
        OutputType::Pdf => "📄 Document",
        _ => "📁 Misc",
    }
}

use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(name = "file_converter_bin")]
#[command(
    author = "File Converter Team",
    version = "0.9.3",
    about = "File Converter CLI & Explorer Context Menu Utility",
    long_about = None
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Preset name to use when converting files
    #[arg(short, long, alias = "conversion-preset")]
    preset: Option<String>,

    /// Path to temporary file containing list of input paths
    #[arg(long)]
    input_files: Option<PathBuf>,

    /// Open settings manager GUI
    #[arg(long, short = 's', alias = "setting")]
    settings: bool,

    /// Input file paths to convert
    #[arg(value_name = "FILES")]
    files: Vec<String>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Convert input files using a specified preset
    Convert {
        /// Conversion preset name (e.g. "To PNG", "To MP3")
        #[arg(short, long)]
        preset: String,

        /// Run headlessly without displaying the progress GUI window
        #[arg(long, default_value_t = false)]
        headless: bool,

        /// Input file paths to convert
        #[arg(required = true, value_name = "FILES")]
        files: Vec<String>,
    },
    /// List all available conversion presets from settings
    ListPresets,
    /// Register shell context menu extension COM DLL
    Register,
    /// Unregister shell context menu extension DLL
    Unregister,
    /// Open the settings GUI configuration window
    Gui,
}

fn create_conversion_jobs(
    preset: &file_converter_core::settings::ConversionPreset,
    input_files: &[String],
) -> Vec<ConversionJob> {
    let total_input_files = input_files.len();
    let mut jobs = Vec::new();
    for (idx, file) in input_files.iter().enumerate() {
        let mut job = ConversionJob::new(idx + 1, preset.clone(), file.clone());
        if let Err(e) = job.prepare(idx, total_input_files) {
            tracing::error!("Failed to prepare job for file {}: {}", job.input_path, e);
        }
        jobs.push(job);
    }
    jobs
}

fn run_headless_conversion(preset_name: &str, input_files: Vec<String>) {
    let settings = match initialize_user_settings_if_needed() {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("Error initializing settings: {}", e);
            std::process::exit(1);
        }
    };

    let preset = match settings
        .conversion_presets
        .iter()
        .find(|p| p.name.eq_ignore_ascii_case(preset_name))
    {
        Some(p) => p.clone(),
        None => {
            tracing::error!("Preset '{}' not found in settings.", preset_name);
            std::process::exit(1);
        }
    };

    let jobs = create_conversion_jobs(&preset, &input_files);

    let scheduler = ConversionScheduler::new(
        jobs,
        settings.maximum_number_of_simultaneous_conversions,
        settings.hardware_acceleration_mode,
        settings.copy_files_in_clipboard_after_conversion,
    );

    println!(
        "Starting headless conversion of {} file(s) using preset '{}'...",
        scheduler.jobs.len(),
        preset_name
    );
    scheduler.execute_all();

    let mut failed = 0;
    for job in &scheduler.jobs {
        let status = job.status.lock();
        match &*status {
            JobStatus::Done => println!("[OK] {}", job.input_path),
            JobStatus::Failed(e) => {
                eprintln!("[FAILED] {}: {}", job.input_path, e);
                failed += 1;
            }
            _ => {}
        }
    }

    if failed > 0 {
        std::process::exit(1);
    }
}

fn normalize_args(args: &[String]) -> Vec<String> {
    args.iter()
        .map(|a| {
            if a == "/preset" || a == "-preset" || a == "-conversion-preset" {
                "--preset".to_string()
            } else if a == "/input-files" || a == "-input-files" {
                "--input-files".to_string()
            } else if a == "/settings" || a == "-settings" {
                "--settings".to_string()
            } else {
                a.clone()
            }
        })
        .collect()
}

fn main() {
    tracing_subscriber::fmt::init();

    let raw_args: Vec<String> = env::args().collect();
    let normalized = normalize_args(&raw_args);

    let cli = match Cli::try_parse_from(&normalized) {
        Ok(c) => c,
        Err(_) => {
            if raw_args.len() <= 1 {
                run_settings_native_gui();
            } else {
                tracing::error!("Invalid command line arguments.");
            }
            return;
        }
    };

    if let Some(cmd) = cli.command {
        match cmd {
            Commands::ListPresets => {
                if let Ok(settings) = initialize_user_settings_if_needed() {
                    println!(
                        "Available Conversion Presets (Total: {}):",
                        settings.conversion_presets.len()
                    );
                    for preset in &settings.conversion_presets {
                        println!(
                            "  • [{}] -> {:?} (Inputs: {})",
                            preset.name,
                            preset.output_type,
                            if preset.input_types.is_empty() {
                                "all".to_string()
                            } else {
                                preset.input_types.join(", ")
                            }
                        );
                    }
                }
                return;
            }
            Commands::Register => {
                println!("{}", register_shell_extension_dll());
                return;
            }
            Commands::Unregister => {
                println!("{}", unregister_shell_extension_dll());
                return;
            }
            Commands::Gui => {
                run_settings_native_gui();
                return;
            }
            Commands::Convert {
                preset,
                headless,
                files,
            } => {
                if headless {
                    run_headless_conversion(&preset, files);
                } else {
                    run_conversion_gui(&preset, files, None);
                }
                return;
            }
        }
    }

    if cli.settings || (cli.preset.is_none() && cli.files.is_empty() && cli.input_files.is_none()) {
        run_settings_native_gui();
        return;
    }

    if let Some(preset_name) = cli.preset {
        let mut input_files = Vec::new();
        let mut temp_to_clean = None;

        if let Some(ref list_path) = cli.input_files {
            if list_path.exists() {
                if let Ok(content) = std::fs::read_to_string(list_path) {
                    for line in content.lines() {
                        let trimmed = line.trim();
                        if !trimmed.is_empty() {
                            input_files.push(trimmed.to_string());
                        }
                    }
                }
                temp_to_clean = Some(list_path.clone());
            }
        }
        input_files.extend(cli.files);

        if input_files.is_empty() {
            println!("Usage: file_converter_bin.exe --preset <PresetName> <file1> <file2> ...");
            return;
        }

        run_conversion_gui(&preset_name, input_files, temp_to_clean);
    } else {
        run_settings_native_gui();
    }
}

fn populate_slint_presets(window: &SettingsWindow, settings: &Settings, selected_idx: usize) {
    let slint_presets: Vec<PresetData> = settings
        .conversion_presets
        .iter()
        .map(|p| PresetData {
            name: p.name.as_str().into(),
            category: get_category_badge(p.output_type).into(),
            output_type: format!("{:?}", p.output_type).into(),
            input_types: p.input_types.join(", ").into(),
            output_file_name_template: p.output_file_name_template.as_str().into(),
            input_post_conversion_action: format!("{:?}", p.input_post_conversion_action).into(),
        })
        .collect();

    window.set_presets(Rc::new(slint::VecModel::from(slint_presets)).into());
    window.set_selected_preset_index(selected_idx as i32);

    if let Some(preset) = settings.conversion_presets.get(selected_idx) {
        window.set_edit_name(preset.name.as_str().into());
        window.set_edit_output_type(format!("{:?}", preset.output_type).into());
        window.set_edit_input_types(preset.input_types.join(", ").into());
        window.set_edit_template(preset.output_file_name_template.as_str().into());
        window.set_edit_post_action(format!("{:?}", preset.input_post_conversion_action).into());

        let preview = file_converter_core::path_helpers::generate_file_path_from_template(
            "C:\\Music\\Album\\sample_track.flac",
            preset.output_type.extension(),
            &preset.output_file_name_template,
            1,
            1,
        );
        window.set_preview_path(preview.into());
    }
}

fn populate_slint_history(window: &SettingsWindow) {
    let history = load_history();
    let slint_history: Vec<HistoryItemData> = history
        .into_iter()
        .map(|h| HistoryItemData {
            timestamp: h.timestamp.into(),
            preset_name: h.preset_name.into(),
            input_path: h.input_path.into(),
            output_path: h.output_path.into(),
            status: h.status.into(),
        })
        .collect();

    window.set_history_items(Rc::new(slint::VecModel::from(slint_history)).into());
}

fn run_settings_native_gui() {
    tracing::info!("Launching File Converter Slint Fluent GUI Settings Window...");

    let window = match SettingsWindow::new() {
        Ok(w) => w,
        Err(e) => {
            tracing::error!("Failed to initialize Slint SettingsWindow: {}", e);
            return;
        }
    };

    let settings = initialize_user_settings_if_needed().unwrap_or_else(|_| Settings {
        serialization_version: 4,
        maximum_number_of_simultaneous_conversions: 4,
        exit_application_when_conversions_finished: true,
        duration_between_end_of_conversions_and_application_exit: 0.0,
        check_upgrade_at_startup: false,
        application_language_name: "en".to_string(),
        copy_files_in_clipboard_after_conversion: true,
        hardware_acceleration_mode: HardwareAccelerationMode::Off,
        auto_start_on_file_drop: true,
        conversion_presets: vec![],
    });
    let check_upgrade = settings.check_upgrade_at_startup;
    let (default_xml_path, user_xml_path) = get_settings_paths();

    let settings_state = Rc::new(std::cell::RefCell::new(settings));
    let user_xml_path_rc = Rc::new(user_xml_path);
    let default_xml_path_rc = Rc::new(default_xml_path);

    // Initial Population
    {
        let s = settings_state.borrow();
        window.set_auto_start_on_file_drop(s.auto_start_on_file_drop);
        window.set_copy_files_in_clipboard_after_conversion(
            s.copy_files_in_clipboard_after_conversion,
        );
        window.set_exit_application_when_conversions_finished(
            s.exit_application_when_conversions_finished,
        );
        populate_slint_presets(&window, &s, 0);
    }
    populate_slint_history(&window);

    // Callback: Save Settings
    let window_weak = window.as_weak();
    let settings_clone = settings_state.clone();
    let xml_path_clone = user_xml_path_rc.clone();
    window.on_save_settings(move || {
        if let Some(w) = window_weak.upgrade() {
            let s = settings_clone.borrow();
            match s.save_to_file(&*xml_path_clone) {
                Ok(_) => w.set_status_msg("Settings saved successfully!".into()),
                Err(e) => w.set_status_msg(format!("Failed to save: {:?}", e).into()),
            }
        }
    });

    // Callbacks: Fast UX Toggles
    let settings_clone = settings_state.clone();
    window.on_toggle_auto_start(move |val| {
        settings_clone.borrow_mut().auto_start_on_file_drop = val;
    });

    let settings_clone = settings_state.clone();
    window.on_toggle_copy_clipboard(move |val| {
        settings_clone
            .borrow_mut()
            .copy_files_in_clipboard_after_conversion = val;
    });

    let settings_clone = settings_state.clone();
    window.on_toggle_auto_close(move |val| {
        let mut s = settings_clone.borrow_mut();
        s.exit_application_when_conversions_finished = val;
        if val {
            s.duration_between_end_of_conversions_and_application_exit = 0.0;
        }
    });

    // Callback: Register Shell
    let window_weak = window.as_weak();
    window.on_register_shell(move || {
        if let Some(w) = window_weak.upgrade() {
            let msg = register_shell_extension_dll();
            w.set_status_msg(msg.into());
        }
    });

    // Callback: Select Preset
    let window_weak = window.as_weak();
    let settings_clone = settings_state.clone();
    window.on_select_preset(move |index| {
        if let Some(w) = window_weak.upgrade() {
            let idx = index as usize;
            populate_slint_presets(&w, &settings_clone.borrow(), idx);
        }
    });

    // Callback: Duplicate Preset
    let window_weak = window.as_weak();
    let settings_clone = settings_state.clone();
    window.on_duplicate_preset(move |index| {
        if let Some(w) = window_weak.upgrade() {
            let mut s = settings_clone.borrow_mut();
            let idx = index as usize;
            if idx < s.conversion_presets.len() {
                let mut cloned = s.conversion_presets[idx].clone();
                cloned.name = format!("{} (Copy)", cloned.name);
                s.conversion_presets.push(cloned);
                let new_idx = s.conversion_presets.len() - 1;
                populate_slint_presets(&w, &s, new_idx);
                w.set_status_msg("Preset duplicated.".into());
            }
        }
    });

    // Callback: New Preset
    let window_weak = window.as_weak();
    let settings_clone = settings_state.clone();
    window.on_new_preset(move || {
        if let Some(w) = window_weak.upgrade() {
            let mut s = settings_clone.borrow_mut();
            let new_preset = file_converter_core::settings::ConversionPreset {
                name: "New Preset".to_string(),
                output_type: file_converter_core::types::OutputType::Png,
                is_default_settings: false,
                input_types: vec![
                    file_converter_core::settings::CompactStr::new("jpg"),
                    file_converter_core::settings::CompactStr::new("bmp"),
                ],
                input_post_conversion_action:
                    file_converter_core::types::InputPostConversionAction::None,
                settings: vec![],
                output_file_name_template: "(p)(f)".to_string(),
            };
            s.conversion_presets.push(new_preset);
            let new_idx = s.conversion_presets.len() - 1;
            populate_slint_presets(&w, &s, new_idx);
            w.set_status_msg("New preset created.".into());
        }
    });

    // Callback: Delete Preset
    let window_weak = window.as_weak();
    let settings_clone = settings_state.clone();
    window.on_delete_preset(move |index| {
        if let Some(w) = window_weak.upgrade() {
            let mut s = settings_clone.borrow_mut();
            let idx = index as usize;
            if s.conversion_presets.len() > 1 && idx < s.conversion_presets.len() {
                let deleted_name = s.conversion_presets.remove(idx).name;
                let new_idx = if idx >= s.conversion_presets.len() {
                    s.conversion_presets.len().saturating_sub(1)
                } else {
                    idx
                };
                populate_slint_presets(&w, &s, new_idx);
                w.set_status_msg(format!("Preset '{}' deleted.", deleted_name).into());
            } else {
                w.set_status_msg("Cannot delete the last remaining preset.".into());
            }
        }
    });

    // Callback: Import Presets
    let window_weak = window.as_weak();
    let settings_clone = settings_state.clone();
    let default_import_path_clone = default_xml_path_rc.clone();
    window.on_import_presets(move || {
        if let Some(w) = window_weak.upgrade() {
            if let Ok(imported_settings) =
                file_converter_core::settings::Settings::load_from_file(&*default_import_path_clone)
            {
                let mut s = settings_clone.borrow_mut();
                s.merge(imported_settings);
                populate_slint_presets(&w, &s, 0);
                w.set_status_msg("Default presets imported and merged.".into());
            } else {
                w.set_status_msg("Failed to read default presets for import.".into());
            }
        }
    });

    // Callback: Export Presets
    let window_weak = window.as_weak();
    let settings_clone = settings_state.clone();
    let export_dir = user_xml_path_rc
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    window.on_export_presets(move || {
        if let Some(w) = window_weak.upgrade() {
            let s = settings_clone.borrow();
            let export_path = export_dir.join("Presets_Export.xml");
            if s.save_to_file(&export_path).is_ok() {
                w.set_status_msg(format!("Exported presets to {}", export_path.display()).into());
            } else {
                w.set_status_msg("Failed to export presets.".into());
            }
        }
    });

    // Callback: Open Update URL
    window.on_open_update_url(move |url| {
        let _ = std::process::Command::new("cmd")
            .args(["/c", "start", url.as_str()])
            .spawn();
    });

    // Callback: Preset Field Edited
    let window_weak = window.as_weak();
    let settings_clone = settings_state.clone();
    window.on_preset_field_changed(move || {
        if let Some(w) = window_weak.upgrade() {
            let mut s = settings_clone.borrow_mut();
            let idx = w.get_selected_preset_index() as usize;
            if let Some(preset) = s.conversion_presets.get_mut(idx) {
                preset.name = w.get_edit_name().to_string();
                preset.output_file_name_template = w.get_edit_template().to_string();
                preset.input_types = w
                    .get_edit_input_types()
                    .split(',')
                    .map(|str| file_converter_core::settings::CompactStr::new(str.trim()))
                    .filter(|str| !str.is_empty())
                    .collect();

                let preview = file_converter_core::path_helpers::generate_file_path_from_template(
                    "C:\\Music\\Album\\sample_track.flac",
                    preset.output_type.extension(),
                    &preset.output_file_name_template,
                    1,
                    1,
                );
                w.set_preview_path(preview.into());
            }
        }
    });

    // Callback: Drop Files
    let window_weak = window.as_weak();
    window.on_drop_files(move || {
        if let Some(w) = window_weak.upgrade() {
            w.set_status_msg(
                "Drop files directly into the window or select a preset to convert.".into(),
            );
        }
    });

    // Check for updates asynchronously in background if enabled
    if check_upgrade {
        let window_weak = window.as_weak();
        std::thread::spawn(move || {
            let current_version = env!("CARGO_PKG_VERSION");
            if let Some(update_info) =
                file_converter_core::update_check::check_for_updates(current_version)
            {
                if update_info.is_newer {
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(w) = window_weak.upgrade() {
                            w.set_update_available_text(
                                format!(
                                    "🎉 New version {} is available!",
                                    update_info.latest_version
                                )
                                .into(),
                            );
                            w.set_update_download_url(update_info.download_url.into());
                        }
                    });
                }
            }
        });
    }

    let _ = window.run();
}

fn run_conversion_gui(
    preset_name: &str,
    input_files: Vec<String>,
    temp_list_to_clean: Option<PathBuf>,
) {
    let settings = match initialize_user_settings_if_needed() {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("Error initializing settings: {}", e);
            return;
        }
    };

    if preset_name.is_empty() || input_files.is_empty() {
        println!("Usage: file_converter_bin.exe --preset <PresetName> <file1> <file2> ...");
        return;
    }

    let preset = match settings
        .conversion_presets
        .iter()
        .find(|p| p.name.eq_ignore_ascii_case(preset_name))
    {
        Some(p) => p.clone(),
        None => {
            tracing::error!("Preset '{}' not found in settings.", preset_name);
            return;
        }
    };

    let jobs = create_conversion_jobs(&preset, &input_files);

    let max_threads = settings.maximum_number_of_simultaneous_conversions;
    let hw_accel = settings.hardware_acceleration_mode;
    let copy_clipboard = settings.copy_files_in_clipboard_after_conversion;

    let scheduler = Arc::new(ConversionScheduler::new(
        jobs,
        max_threads,
        hw_accel,
        copy_clipboard,
    ));

    let scheduler_clone = Arc::clone(&scheduler);
    thread::spawn(move || {
        scheduler_clone.execute_all();
    });

    let window = match ProgressWindow::new() {
        Ok(w) => w,
        Err(e) => {
            eprintln!("Failed to initialize ProgressWindow: {}", e);
            return;
        }
    };

    window.set_preset_name(preset_name.into());
    window.set_overall_progress(0.0);
    window.set_overall_status_text("Starting conversion...".into());

    let scheduler_rc = scheduler.clone();
    let auto_close = settings.exit_application_when_conversions_finished;
    let exit_delay = settings.duration_between_end_of_conversions_and_application_exit;
    let _start_time = Instant::now();
    let finished_flag = Rc::new(std::cell::Cell::new(false));
    let close_time_flag = Rc::new(std::cell::RefCell::new(None::<Instant>));

    let preset_name_clone = preset_name.to_string();

    // Slint Timer for Live UI Progress Updates (100ms interval)
    let timer = slint::Timer::default();
    let window_weak = window.as_weak();
    let scheduler_timer = scheduler_rc.clone();
    let finished_flag_timer = finished_flag.clone();
    let close_time_timer = close_time_flag.clone();

    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(100),
        move || {
            if let Some(w) = window_weak.upgrade() {
                let mut total_prog = 0.0f32;
                let mut completed_count = 0;
                let mut failed_count = 0;
                let total_count = scheduler_timer.jobs.len();

                let mut job_models = Vec::new();

                for job in &scheduler_timer.jobs {
                    let p = job.get_progress();
                    total_prog += p;

                    let s = job.status.lock().clone();
                    let filename = Path::new(&job.input_path)
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| job.input_path.clone());

                    let (status_text, is_done, is_failed) = match &s {
                        JobStatus::Queue => ("Queued...".to_string(), false, false),
                        JobStatus::Converting(msg) => {
                            (format!("Converting ({})", msg), false, false)
                        }
                        JobStatus::Done => {
                            completed_count += 1;
                            ("Done".to_string(), true, false)
                        }
                        JobStatus::Failed(err) => {
                            completed_count += 1;
                            failed_count += 1;
                            (format!("Error: {}", err), false, true)
                        }
                        JobStatus::Canceled => {
                            completed_count += 1;
                            ("Canceled".to_string(), false, true)
                        }
                    };

                    job_models.push(JobProgressData {
                        id: job.id as i32,
                        input_file_name: filename.into(),
                        input_path: job.input_path.as_str().into(),
                        output_path: job.output_file_paths.join("; ").into(),
                        progress: p,
                        status_text: status_text.into(),
                        is_done,
                        is_failed,
                    });
                }

                let overall = if total_count > 0 {
                    total_prog / total_count as f32
                } else {
                    1.0
                };

                let status_summary = if completed_count >= total_count {
                    if failed_count > 0 {
                        format!("Completed with {} failure(s)", failed_count)
                    } else {
                        "All conversions completed successfully!".to_string()
                    }
                } else {
                    format!(
                        "Converting {} of {} file(s)...",
                        (completed_count + 1).min(total_count),
                        total_count
                    )
                };

                w.set_overall_status_text(status_summary.into());
                w.set_overall_progress(overall);
                w.set_jobs(Rc::new(slint::VecModel::from(job_models)).into());

                if completed_count >= total_count {
                    if !finished_flag_timer.get() {
                        finished_flag_timer.set(true);
                        *close_time_timer.borrow_mut() = Some(Instant::now());
                        play_completion_sound();

                        for job in &scheduler_timer.jobs {
                            let out_str = job.output_file_paths.join("; ");
                            let status_guard = job.status.lock();
                            let status_str = match &*status_guard {
                                JobStatus::Done => "Done".to_string(),
                                JobStatus::Failed(e) => format!("Failed ({})", e),
                                JobStatus::Canceled => "Canceled".to_string(),
                                _ => "Finished".to_string(),
                            };
                            add_history_record(
                                &preset_name_clone,
                                &job.input_path,
                                &out_str,
                                &status_str,
                            );
                        }
                    }

                    w.set_is_finished(true);
                    if auto_close {
                        if let Some(start) = *close_time_timer.borrow() {
                            let elapsed = start.elapsed().as_secs_f32();
                            if elapsed >= exit_delay {
                                slint::quit_event_loop().unwrap();
                            }
                        }
                    }
                }
            }
        },
    );

    // Callbacks
    let scheduler_cancel = scheduler_rc.clone();
    window.on_cancel_job(move |job_id| {
        for job in &scheduler_cancel.jobs {
            if job.id == job_id as usize {
                job.cancel();
            }
        }
    });

    let scheduler_folder = scheduler_rc.clone();
    window.on_open_output_folder(move || {
        if let Some(first_job) = scheduler_folder.jobs.first() {
            if let Some(first_out) = first_job.output_file_paths.first() {
                let parent = Path::new(first_out)
                    .parent()
                    .unwrap_or_else(|| Path::new("."));
                let _ = std::process::Command::new("explorer").arg(parent).spawn();
            }
        }
    });

    let scheduler_copy = scheduler_rc.clone();
    window.on_copy_output_paths(move || {
        let mut all_outs = Vec::new();
        for job in &scheduler_copy.jobs {
            for out in &job.output_file_paths {
                all_outs.push(out.clone());
            }
        }
        if !all_outs.is_empty() {
            let _ = file_converter_core::scheduler::copy_files_to_clipboard(&all_outs);
        }
    });

    let window_close_cb = window.as_weak();
    window.on_close_window(move || {
        if window_close_cb.upgrade().is_some() {
            slint::quit_event_loop().unwrap();
        }
    });

    let _ = window.run();

    if let Some(temp_path) = temp_list_to_clean {
        let _ = std::fs::remove_file(temp_path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_embedded_default_settings_xml_syntax() {
        assert!(!DEFAULT_SETTINGS_XML.is_empty());
        let settings = Settings::load_from_str(DEFAULT_SETTINGS_XML);
        assert!(settings.is_ok());
        let s = settings.unwrap();
        assert!(!s.conversion_presets.is_empty());
    }

    #[test]
    fn test_settings_paths_resolution() {
        let (default_xml, user_xml) = get_settings_paths();
        assert!(
            default_xml
                .to_string_lossy()
                .contains("Settings.default.xml")
        );
        assert!(user_xml.to_string_lossy().contains("Settings.user.xml"));
    }
}
