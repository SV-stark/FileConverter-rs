#![allow(clippy::all, warnings)]
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

    let status = std::process::Command::new("regsvr32.exe")
        .arg("/s")
        .arg(&dll_path)
        .status();

    match status {
        Ok(s) if s.success() => "Shell extension context menu registered successfully!".to_string(),
        Ok(s) => format!("regsvr32 failed with exit code: {:?}", s.code()),
        Err(e) => format!("Failed to run regsvr32: {:?}", e),
    }
}

fn main() {
    let args: Vec<String> = env::args().collect();

    let run_gui = args.len() < 2
        || args
            .iter()
            .any(|arg| arg == "-settings" || arg == "/settings");

    if run_gui {
        run_settings_native_gui();
    } else {
        run_conversion_gui(args);
    }
}

struct FileConverterApp {
    settings: Settings,
    user_xml_path: PathBuf,
    selected_preset_index: usize,
    status_msg: String,
}

impl eframe::App for FileConverterApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // Top Header Frame
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.heading("⚡ File Converter Settings");
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
                });
            });

            ui.separator();

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
        });

        ui.add_space(6.0);

        // Main 2-column Layout (Preset List on Left, Active Preset Config on Right)
        ui.columns(2, |columns| {
            // Left Panel: Preset List & Actions
            columns[0].group(|ui| {
                ui.heading("Presets List");
                ui.separator();

                egui::ScrollArea::vertical()
                    .max_height(360.0)
                    .show(ui, |ui| {
                        let len = self.settings.conversion_presets.len();
                        for i in 0..len {
                            let name = self.settings.conversion_presets[i].name.clone();
                            let is_selected = i == self.selected_preset_index;
                            let label = if name.is_empty() {
                                "Unnamed Preset"
                            } else {
                                &name
                            };

                            if ui.selectable_label(is_selected, label).clicked() {
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
                        if self.selected_preset_index + 1 < self.settings.conversion_presets.len() {
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
                    let preset = &mut self.settings.conversion_presets[self.selected_preset_index];

                    ui.heading(format!("Edit: {}", preset.name));
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

                    // File Template Sample Preview (Matches WPF SettingsWindow.xaml)
                    let ext_str = format!("{:?}", preset.output_type).to_lowercase();
                    let sample_output =
                        format!("C:\\ConvertedFiles\\MyDocument_converted.{}", ext_str);
                    ui.label("File Name Sample Preview:");
                    ui.label(
                        egui::RichText::new(format!("Example: {}", sample_output))
                            .italics()
                            .weak(),
                    );

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

        ui.add_space(5.0);
        ui.separator();
        ui.label(&self.status_msg);
    }
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

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([880.0, 620.0])
            .with_title("File Converter Native Settings"),
        ..Default::default()
    };

    let app = FileConverterApp {
        settings,
        user_xml_path,
        selected_preset_index: 0,
        status_msg: "Ready".to_string(),
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
            .max_height(300.0)
            .show(ui, |ui| {
                for job in &self.scheduler.jobs {
                    let p = *job.progress.lock().unwrap();
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
            }

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

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([650.0, 450.0])
            .with_title(format!("File Converter - {}", preset_name)),
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
