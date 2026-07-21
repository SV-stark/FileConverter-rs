#![allow(clippy::collapsible_if)]
#![windows_subsystem = "windows"]
use std::env;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use eframe::egui;
use file_converter_core::scheduler::{ConversionJob, ConversionScheduler, JobStatus};
use file_converter_core::settings::{ConversionPreset, Settings};
use file_converter_core::types::{HardwareAccelerationMode, InputPostConversionAction, OutputType};

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
    version = "0.4.0",
    about = "File Converter CLI & Explorer Context Menu Utility",
    long_about = None
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Preset name to use when converting files
    #[arg(short, long)]
    preset: Option<String>,

    /// Path to temporary file containing list of input paths
    #[arg(long)]
    input_files: Option<PathBuf>,

    /// Open settings manager GUI
    #[arg(long, default_value_t = false)]
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
    let cli = Cli::parse();

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

    if cli.settings || raw_args.len() < 2 {
        run_settings_native_gui();
    } else {
        run_conversion_gui(raw_args);
    }
}

#[derive(PartialEq, Eq)]
enum AppTab {
    Settings,
    History,
}

struct FileConverterApp {
    settings: Settings,
    user_xml_path: PathBuf,
    selected_preset_index: usize,
    status_msg: String,
    dark_mode: bool,
    preset_search_query: String,
    active_tab: AppTab,
    history: Vec<HistoryRecord>,
}

impl eframe::App for FileConverterApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // Handle Drag & Drop Files
        let dropped_files = ui.ctx().input(|i| i.raw.dropped_files.clone());
        if !dropped_files.is_empty() {
            let file_paths: Vec<String> = dropped_files
                .iter()
                .filter_map(|f| f.path.as_ref().map(|p| p.to_string_lossy().to_string()))
                .collect();
            if !file_paths.is_empty()
                && self.selected_preset_index < self.settings.conversion_presets.len()
            {
                let preset_name = self.settings.conversion_presets[self.selected_preset_index]
                    .name
                    .clone();
                let mut cmd_args = vec![
                    "fcrs".to_string(),
                    "--conversion-preset".to_string(),
                    preset_name,
                ];
                cmd_args.extend(file_paths);
                thread::spawn(move || {
                    run_conversion_gui(cmd_args);
                });
            }
        }

        // Top Header Frame
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.heading("⚡ File Converter Settings");

                ui.separator();
                if ui
                    .selectable_label(self.active_tab == AppTab::Settings, "⚙️ Settings")
                    .clicked()
                {
                    self.active_tab = AppTab::Settings;
                }
                if ui
                    .selectable_label(
                        self.active_tab == AppTab::History,
                        format!("📜 History ({})", self.history.len()),
                    )
                    .clicked()
                {
                    self.history = load_history();
                    self.active_tab = AppTab::History;
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("💾 Save Settings").clicked() {
                        match self.settings.save_to_file(&self.user_xml_path) {
                            Ok(_) => self.status_msg = "Settings saved successfully!".to_string(),
                            Err(e) => self.status_msg = format!("Failed to save: {:?}", e),
                        }
                    }
                    if ui.button("⚙️ Register Shell Extension").clicked() {
                        self.status_msg = register_shell_extension_dll();
                    }
                    if ui.selectable_label(self.dark_mode, "🌙 Dark").clicked() {
                        self.dark_mode = true;
                        ui.ctx().set_visuals(egui::Visuals::dark());
                    }
                    if ui.selectable_label(!self.dark_mode, "☀️ Light").clicked() {
                        self.dark_mode = false;
                        ui.ctx().set_visuals(egui::Visuals::light());
                    }
                });
            });

            if self.active_tab == AppTab::Settings {
                ui.separator();

                // Drag and Drop Zone
                egui::Frame::group(ui.style())
                    .fill(if self.dark_mode {
                        egui::Color32::from_rgb(30, 35, 45)
                    } else {
                        egui::Color32::from_rgb(240, 245, 250)
                    })
                    .show(ui, |ui| {
                        ui.vertical_centered(|ui| {
                            ui.add_space(4.0);
                            ui.label(
                                egui::RichText::new(
                                    "📂 Drag & Drop Files Here to Convert Directly",
                                )
                                .strong(),
                            );
                            ui.add_space(4.0);
                        });
                    });

                ui.horizontal(|ui| {
                    ui.label("Max Concurrency:");
                    ui.add(
                        egui::DragValue::new(
                            &mut self.settings.maximum_number_of_simultaneous_conversions,
                        )
                        .range(1..=32),
                    );

                    ui.separator();
                    ui.checkbox(
                        &mut self.settings.copy_files_in_clipboard_after_conversion,
                        "Copy output files to Clipboard",
                    );

                    ui.separator();
                    ui.label("Hardware Acceleration:");
                    egui::ComboBox::from_id_salt("hw_accel")
                        .selected_text(format!("{:?}", self.settings.hardware_acceleration_mode))
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut self.settings.hardware_acceleration_mode,
                                HardwareAccelerationMode::Off,
                                "Off (CPU)",
                            );
                            ui.selectable_value(
                                &mut self.settings.hardware_acceleration_mode,
                                HardwareAccelerationMode::Cuda,
                                "NVIDIA (CUDA)",
                            );
                            ui.selectable_value(
                                &mut self.settings.hardware_acceleration_mode,
                                HardwareAccelerationMode::Amf,
                                "AMD (AMF)",
                            );
                        });
                });
            }
        });

        ui.add_space(6.0);

        if self.active_tab == AppTab::History {
            // Render Conversion History Log Tab
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.heading("📜 Recent Conversions History Log");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("🗑️ Clear History").clicked() {
                            self.history.clear();
                            save_history(&self.history);
                        }
                    });
                });
                ui.separator();

                if self.history.is_empty() {
                    ui.label("No recent conversions logged yet.");
                } else {
                    egui::ScrollArea::vertical()
                        .max_height(420.0)
                        .show(ui, |ui| {
                            egui::Grid::new("history_grid")
                                .striped(true)
                                .num_columns(5)
                                .spacing([12.0, 6.0])
                                .show(ui, |ui| {
                                    ui.label(egui::RichText::new("Timestamp").strong());
                                    ui.label(egui::RichText::new("Preset").strong());
                                    ui.label(egui::RichText::new("Input Path").strong());
                                    ui.label(egui::RichText::new("Output Path").strong());
                                    ui.label(egui::RichText::new("Status").strong());
                                    ui.end_row();

                                    for item in &self.history {
                                        ui.label(&item.timestamp);
                                        ui.label(&item.preset_name);
                                        ui.label(&item.input_path);
                                        ui.label(&item.output_path);
                                        ui.label(&item.status);
                                        ui.end_row();
                                    }
                                });
                        });
                }
            });
        } else {
            // Main 2-column Layout (Preset List on Left, Active Preset Config on Right)
            ui.columns(2, |columns| {
                // Left Panel: Preset List & Actions
                columns[0].group(|ui| {
                    ui.horizontal(|ui| {
                        ui.heading("Presets List");
                    });
                    ui.separator();

                    ui.horizontal(|ui| {
                        ui.label("🔍");
                        ui.text_edit_singleline(&mut self.preset_search_query);
                        if !self.preset_search_query.is_empty() && ui.button("✖").clicked() {
                            self.preset_search_query.clear();
                        }
                    });
                    ui.separator();

                    egui::ScrollArea::vertical()
                        .max_height(320.0)
                        .show(ui, |ui| {
                            let len = self.settings.conversion_presets.len();
                            let q = self.preset_search_query.to_lowercase();
                            for i in 0..len {
                                let preset = &self.settings.conversion_presets[i];
                                let name = preset.name.clone();
                                let badge = get_category_badge(preset.output_type);

                                if !q.is_empty()
                                    && !name.to_lowercase().contains(&q)
                                    && !badge.to_lowercase().contains(&q)
                                {
                                    continue;
                                }

                                let is_selected = i == self.selected_preset_index;
                                let label_text = format!(
                                    "{} {}",
                                    badge,
                                    if name.is_empty() {
                                        "Unnamed Preset"
                                    } else {
                                        &name
                                    }
                                );

                                if ui.selectable_label(is_selected, label_text).clicked() {
                                    self.selected_preset_index = i;
                                }
                            }
                        });

                    ui.separator();
                    ui.horizontal(|ui| {
                        if ui.button("➕ Add").clicked() {
                            let new_preset = ConversionPreset {
                                name: "New Preset".to_string(),
                                output_type: OutputType::Png,
                                output_file_name_template: "(p)\\(f)".to_string(),
                                is_default_settings: false,
                                input_types: vec![],
                                input_post_conversion_action: InputPostConversionAction::None,
                                settings: vec![],
                            };
                            self.settings.conversion_presets.push(new_preset);
                            self.selected_preset_index = self.settings.conversion_presets.len() - 1;
                        }

                        if self.selected_preset_index < self.settings.conversion_presets.len() {
                            if ui.button("📋 Duplicate").clicked() {
                                let mut cloned = self.settings.conversion_presets
                                    [self.selected_preset_index]
                                    .clone();
                                cloned.name = format!("{} (Copy)", cloned.name);
                                self.settings.conversion_presets.push(cloned);
                                self.selected_preset_index =
                                    self.settings.conversion_presets.len() - 1;
                                self.status_msg = "Preset duplicated.".to_string();
                            }

                            if ui.button("🗑️ Delete").clicked() {
                                self.settings
                                    .conversion_presets
                                    .remove(self.selected_preset_index);
                                if self.selected_preset_index > 0 {
                                    self.selected_preset_index -= 1;
                                }
                            }
                            if self.selected_preset_index > 0 {
                                if ui.button("⬆️ Up").clicked() {
                                    self.settings.conversion_presets.swap(
                                        self.selected_preset_index,
                                        self.selected_preset_index - 1,
                                    );
                                    self.selected_preset_index -= 1;
                                }
                            }
                            if self.selected_preset_index + 1
                                < self.settings.conversion_presets.len()
                            {
                                if ui.button("⬇️ Down").clicked() {
                                    self.settings.conversion_presets.swap(
                                        self.selected_preset_index,
                                        self.selected_preset_index + 1,
                                    );
                                    self.selected_preset_index += 1;
                                }
                            }
                        }
                    });
                });

                // Right Panel: Selected Preset Details & Form Fields
                columns[1].group(|ui| {
                    if self.selected_preset_index < self.settings.conversion_presets.len() {
                        let preset =
                            &mut self.settings.conversion_presets[self.selected_preset_index];

                        let badge = get_category_badge(preset.output_type);
                        ui.heading(format!("{} Edit: {}", badge, preset.name));
                        ui.separator();

                        egui::Grid::new("preset_fields_grid")
                            .num_columns(2)
                            .spacing([12.0, 8.0])
                            .show(ui, |ui| {
                                ui.label("Preset Name:");
                                ui.text_edit_singleline(&mut preset.name);
                                ui.end_row();

                                ui.label("Output Format:");
                                ui.label(format!("{:?}", preset.output_type));
                                ui.end_row();

                                ui.label("Path Template:");
                                ui.text_edit_singleline(&mut preset.output_file_name_template);
                                ui.end_row();
                            });

                        ui.separator();

                        // Real-time Path Template Preview Box
                        let ext_str = preset.output_type.extension();
                        let sample_input = "C:\\SampleMedia\\MyDocument.flac";
                        let live_preview =
                            file_converter_core::path_helpers::generate_file_path_from_template(
                                sample_input,
                                ext_str,
                                &preset.output_file_name_template,
                                1,
                                1,
                            );

                        egui::Frame::group(ui.style())
                            .fill(if self.dark_mode {
                                egui::Color32::from_rgb(25, 30, 40)
                            } else {
                                egui::Color32::from_rgb(245, 248, 252)
                            })
                            .show(ui, |ui| {
                                ui.label(
                                    egui::RichText::new("✨ Live Output Path Template Preview")
                                        .strong(),
                                );
                                ui.label(
                                    egui::RichText::new(format!("Input Sample:  {}", sample_input))
                                        .weak(),
                                );
                                ui.label(
                                    egui::RichText::new(format!("Output Result: {}", live_preview))
                                        .strong()
                                        .color(if self.dark_mode {
                                            egui::Color32::from_rgb(100, 200, 255)
                                        } else {
                                            egui::Color32::from_rgb(0, 100, 200)
                                        }),
                                );
                            });

                        ui.separator();

                        // Post-Conversion Action Selector
                        ui.horizontal(|ui| {
                            ui.label("Post-Conversion Action:");
                            egui::ComboBox::from_id_salt("post_action")
                                .selected_text(format!("{:?}", preset.input_post_conversion_action))
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(
                                        &mut preset.input_post_conversion_action,
                                        InputPostConversionAction::None,
                                        "None (Keep Original)",
                                    );
                                    ui.selectable_value(
                                        &mut preset.input_post_conversion_action,
                                        InputPostConversionAction::Delete,
                                        "Delete Original File",
                                    );
                                });
                        });

                        ui.separator();

                        ui.label("Input File Extensions (comma separated):");
                        let mut input_str = preset.input_types.join(", ");
                        if ui.text_edit_singleline(&mut input_str).changed() {
                            preset.input_types = input_str
                                .split(',')
                                .map(|s| s.trim().to_string())
                                .filter(|s| !s.is_empty())
                                .collect();
                        }
                    } else {
                        ui.label("Select a preset on the left panel to configure its options.");
                    }
                });
            });
        }

        ui.add_space(5.0);
        ui.separator();
        ui.label(&self.status_msg);
    }
}

fn load_app_icon() -> Option<egui::IconData> {
    eframe::icon_data::from_png_bytes(include_bytes!("../../icon.png")).ok()
}

fn run_settings_native_gui() {
    println!("Launching File Converter Native GUI Settings Window...");

    let settings = match initialize_user_settings_if_needed() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Error initializing settings: {}", e);
            return;
        }
    };
    let (_, user_xml_path) = get_settings_paths();

    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([880.0, 620.0])
        .with_title("File Converter Native Settings");

    if let Some(icon) = load_app_icon() {
        viewport = viewport.with_icon(icon);
    }

    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    let app = FileConverterApp {
        settings,
        user_xml_path,
        selected_preset_index: 0,
        status_msg: "Ready".to_string(),
        dark_mode: true,
        preset_search_query: String::new(),
        active_tab: AppTab::Settings,
        history: load_history(),
    };

    let _ = eframe::run_native(
        "File Converter Settings",
        options,
        Box::new(|_cc| Ok(Box::new(app))),
    );
}

struct ProgressApp {
    scheduler: Arc<ConversionScheduler>,
    preset_name: String,
    auto_close: bool,
    exit_delay: f32,
    finished: bool,
    close_time: Option<std::time::Instant>,
}

impl eframe::App for ProgressApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        ui.ctx().request_repaint_after(Duration::from_millis(100));

        let mut total_prog = 0.0f32;
        let mut completed_count = 0;
        let total_count = self.scheduler.jobs.len();

        ui.heading(format!("⚡ Converting via '{}'...", self.preset_name));
        ui.separator();

        egui::ScrollArea::vertical()
            .max_height(280.0)
            .show(ui, |ui| {
                for job in &self.scheduler.jobs {
                    let p = job.get_progress();
                    let s = job.status.lock().unwrap().clone();

                    total_prog += p;

                    let filename = Path::new(&job.input_path)
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| job.input_path.clone());

                    let status_str = match &s {
                        JobStatus::Queue => "Queued...".to_string(),
                        JobStatus::Converting(msg) => format!("Converting ({})", msg),
                        JobStatus::Done => {
                            completed_count += 1;
                            "Done".to_string()
                        }
                        JobStatus::Failed(err) => {
                            completed_count += 1;
                            format!("Failed: {}", err)
                        }
                        JobStatus::Canceled => {
                            completed_count += 1;
                            "Canceled".to_string()
                        }
                    };

                    ui.horizontal(|ui| {
                        ui.label(&filename);
                        ui.add(egui::ProgressBar::new(p).text(&status_str));
                    });
                }
            });

        ui.separator();

        let overall = if total_count > 0 {
            total_prog / total_count as f32
        } else {
            1.0
        };

        ui.horizontal(|ui| {
            ui.label("Overall Progress:");
            ui.add(egui::ProgressBar::new(overall).text(format!(
                "{}/{} finished ({:.0}%)",
                completed_count,
                total_count,
                overall * 100.0
            )));
        });

        if completed_count >= total_count {
            if !self.finished {
                self.finished = true;
                self.close_time = Some(std::time::Instant::now());
                play_completion_sound();

                // Log all jobs to History
                for job in &self.scheduler.jobs {
                    let out_str = job.output_file_paths.join("; ");
                    let status_str = match &*job.status.lock().unwrap() {
                        JobStatus::Done => "Done".to_string(),
                        JobStatus::Failed(e) => format!("Failed ({})", e),
                        JobStatus::Canceled => "Canceled".to_string(),
                        _ => "Finished".to_string(),
                    };
                    add_history_record(&self.preset_name, &job.input_path, &out_str, &status_str);
                }
            }

            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("📁 Open Output Folder").clicked() {
                    if let Some(first_job) = self.scheduler.jobs.first() {
                        if let Some(first_out) = first_job.output_file_paths.first() {
                            let parent = Path::new(first_out)
                                .parent()
                                .unwrap_or_else(|| Path::new("."));
                            let _ = std::process::Command::new("explorer").arg(parent).spawn();
                        }
                    }
                }

                if ui.button("📋 Copy Output Paths").clicked() {
                    let mut all_paths = Vec::new();
                    for job in &self.scheduler.jobs {
                        all_paths.extend(job.output_file_paths.clone());
                    }
                    ui.ctx().copy_text(all_paths.join("\n"));
                }
            });

            if self.auto_close {
                if let Some(start) = self.close_time {
                    let elapsed = start.elapsed().as_secs_f32();
                    let remaining = (self.exit_delay - elapsed).max(0.0);
                    ui.label(format!("Finished! Closing in {:.1}s...", remaining));
                    if elapsed >= self.exit_delay {
                        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                }
            } else {
                ui.label("Conversions complete.");
            }
        }
    }
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
            || arg == "--conversion-preset")
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

    let auto_close = settings.exit_application_when_conversions_finished;
    let exit_delay = settings.duration_between_end_of_conversions_and_application_exit;

    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([650.0, 450.0])
        .with_title(format!("File Converter - {}", preset_name));

    if let Some(icon) = load_app_icon() {
        viewport = viewport.with_icon(icon);
    }

    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    let app = ProgressApp {
        scheduler,
        preset_name,
        auto_close,
        exit_delay,
        finished: false,
        close_time: None,
    };

    let _ = eframe::run_native(
        "File Converter Progress",
        options,
        Box::new(|_cc| Ok(Box::new(app))),
    );

    if let Some(temp_path) = temp_list_to_clean {
        let _ = std::fs::remove_file(temp_path);
    }
}
