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
    let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
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
    version = "0.7.0",
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

fn run_headless_conversion(preset_name: &str, input_files: Vec<String>) {
    let settings = match initialize_user_settings_if_needed() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Error initializing settings: {}", e);
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
            eprintln!("Preset '{}' not found in settings.", preset_name);
            std::process::exit(1);
        }
    };

    let total_input_files = input_files.len();
    let mut jobs = Vec::new();
    for (idx, file) in input_files.into_iter().enumerate() {
        let mut job = ConversionJob::new(idx + 1, preset.clone(), file);
        if let Err(e) = job.prepare(idx, total_input_files) {
            eprintln!("Failed to prepare job for file {}: {}", job.input_path, e);
        }
        jobs.push(job);
    }

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
        let status = job.status.lock().unwrap();
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

fn main() {
    let raw_args: Vec<String> = env::args().collect();

    // Check if invoked via standard clap CLI
    if let Ok(cli) = Cli::try_parse() {
        #[allow(clippy::collapsible_match)]
        match cli.command {
            Some(Commands::ListPresets) => {
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
            Some(Commands::Register) => {
                println!("{}", register_shell_extension_dll());
                return;
            }
            Some(Commands::Unregister) => {
                println!("{}", unregister_shell_extension_dll());
                return;
            }
            Some(Commands::Gui) => {
                run_settings_native_gui();
                return;
            }
            Some(Commands::Convert {
                preset,
                headless,
                files,
            }) => {
                if headless {
                    run_headless_conversion(&preset, files);
                    return;
                }
            }
            None => {}
        }

        if cli.settings {
            run_settings_native_gui();
            return;
        }
    }

    let is_settings_arg = raw_args.iter().skip(1).any(|a| {
        a.eq_ignore_ascii_case("-settings")
            || a.eq_ignore_ascii_case("--settings")
            || a.eq_ignore_ascii_case("/settings")
    });

    if is_settings_arg || raw_args.len() < 2 {
        run_settings_native_gui();
    } else {
        run_conversion_gui(raw_args);
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
    println!("Launching File Converter Slint Fluent GUI Settings Window...");

    let window = match SettingsWindow::new() {
        Ok(w) => w,
        Err(e) => {
            eprintln!("Failed to initialize Slint SettingsWindow: {}", e);
            return;
        }
    };

    let settings = initialize_user_settings_if_needed().unwrap_or_else(|_| Settings {
        serialization_version: 4,
        maximum_number_of_simultaneous_conversions: 4,
        exit_application_when_conversions_finished: true,
        duration_between_end_of_conversions_and_application_exit: 2.0,
        check_upgrade_at_startup: false,
        application_language_name: "en".to_string(),
        copy_files_in_clipboard_after_conversion: true,
        hardware_acceleration_mode: HardwareAccelerationMode::Off,
        conversion_presets: vec![],
    });
    let (_, user_xml_path) = get_settings_paths();

    let settings_state = Rc::new(std::cell::RefCell::new(settings));
    let user_xml_path_rc = Rc::new(user_xml_path);

    // Initial Population
    populate_slint_presets(&window, &settings_state.borrow(), 0);
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
                    .map(|str| str.trim().to_string())
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

    let _ = window.run();
}

fn run_conversion_gui(args: Vec<String>) {
    let settings = match initialize_user_settings_if_needed() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Error initializing settings: {}", e);
            return;
        }
    };

    let mut preset_name = String::new();
    let mut input_files = Vec::new();
    let mut temp_list_to_clean: Option<PathBuf> = None;

    let mut i = 1;
    while i < args.len() {
        let arg = &args[i];
        if (arg == "-preset"
            || arg == "/preset"
            || arg == "--preset"
            || arg == "--conversion-preset"
            || arg == "-conversion-preset")
            && i + 1 < args.len()
        {
            preset_name = args[i + 1].clone();
            i += 2;
        } else if (arg == "--input-files" || arg == "-input-files" || arg == "/input-files")
            && i + 1 < args.len()
        {
            let list_path = PathBuf::from(&args[i + 1]);
            if list_path.exists() {
                if let Ok(content) = std::fs::read_to_string(&list_path) {
                    for line in content.lines() {
                        let trimmed = line.trim();
                        if !trimmed.is_empty() {
                            input_files.push(trimmed.to_string());
                        }
                    }
                }
                temp_list_to_clean = Some(list_path);
            }
            i += 2;
        } else if arg == "-settings" || arg == "--settings" || arg == "/settings" {
            i += 1;
        } else {
            input_files.push(args[i].clone());
            i += 1;
        }
    }

    if preset_name.is_empty() || input_files.is_empty() {
        println!("Usage: file_converter_bin.exe -preset <PresetName> <file1> <file2> ...");
        return;
    }

    let preset = match settings
        .conversion_presets
        .iter()
        .find(|p| p.name.eq_ignore_ascii_case(&preset_name))
    {
        Some(p) => p.clone(),
        None => {
            eprintln!("Preset '{}' not found in settings.", preset_name);
            return;
        }
    };

    let total_input_files = input_files.len();
    let mut jobs = Vec::new();
    for (idx, file) in input_files.into_iter().enumerate() {
        let mut job = ConversionJob::new(idx + 1, preset.clone(), file);
        if let Err(e) = job.prepare(idx, total_input_files) {
            eprintln!("Failed to prepare job for file {}: {}", job.input_path, e);
        }
        jobs.push(job);
    }

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

    window.set_preset_name(preset_name.as_str().into());
    window.set_overall_progress(0.0);

    let scheduler_rc = scheduler.clone();
    let auto_close = settings.exit_application_when_conversions_finished;
    let exit_delay = settings.duration_between_end_of_conversions_and_application_exit;
    let _start_time = Instant::now();
    let finished_flag = Rc::new(std::cell::Cell::new(false));
    let close_time_flag = Rc::new(std::cell::RefCell::new(None::<Instant>));

    let preset_name_clone = preset_name.clone();

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
                let mut _failed_count = 0;
                let total_count = scheduler_timer.jobs.len();

                let mut job_models = Vec::new();

                for job in &scheduler_timer.jobs {
                    let p = job.get_progress();
                    total_prog += p;

                    let s = job.status.lock().unwrap().clone();
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
                            _failed_count += 1;
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

                w.set_overall_progress(overall);
                w.set_jobs(Rc::new(slint::VecModel::from(job_models)).into());

                if completed_count >= total_count {
                    if !finished_flag_timer.get() {
                        finished_flag_timer.set(true);
                        *close_time_timer.borrow_mut() = Some(Instant::now());
                        play_completion_sound();

                        for job in &scheduler_timer.jobs {
                            let out_str = job.output_file_paths.join("; ");
                            let status_str = match &*job.status.lock().unwrap() {
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
