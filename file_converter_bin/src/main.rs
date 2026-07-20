#![allow(clippy::all, warnings)]
use std::env;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use eframe::egui;
use file_converter_core::scheduler::{ConversionJob, ConversionScheduler};
use file_converter_core::settings::{ConversionPreset, Settings};
use file_converter_core::types::{InputPostConversionAction, OutputType};

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

fn initialize_user_settings_if_needed() -> Result<Settings, String> {
    let (default_xml, user_xml) = get_settings_paths();

    if !user_xml.exists() {
        if let Some(parent) = user_xml.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        if default_xml.exists() {
            let _ = std::fs::copy(&default_xml, &user_xml);
        } else {
            let basic_default = r#"<?xml version="1.0" encoding="utf-8"?>
<Settings xmlns:xsd="http://www.w3.org/2001/XMLSchema" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" SerializationVersion="4">
  <MaximumNumberOfSimultaneousConversions>2</MaximumNumberOfSimultaneousConversions>
  <ExitApplicationWhenConversionsFinished>true</ExitApplicationWhenConversionsFinished>
  <DurationBetweenEndOfConversionsAndApplicationExit>2</DurationBetweenEndOfConversionsAndApplicationExit>
  <CheckUpgradeAtStartup>true</CheckUpgradeAtStartup>
  <ApplicationLanguageName>en</ApplicationLanguageName>
  <CopyFilesInClipboardAfterConversion>true</CopyFilesInClipboardAfterConversion>
  <HardwareAccelerationMode>Off</HardwareAccelerationMode>
</Settings>"#;
            let _ = std::fs::write(&user_xml, basic_default);
        }
    }

    Settings::load_from_file(&user_xml).map_err(|e| format!("Failed to load settings: {:?}", e))
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
        run_cli_conversions(args);
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
        ui.horizontal(|ui| {
            ui.heading("⚡ File Converter Native Settings");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("💾 Save Settings").clicked() {
                    match self.settings.save_to_file(&self.user_xml_path) {
                        Ok(_) => self.status_msg = "Settings saved successfully!".to_string(),
                        Err(e) => self.status_msg = format!("Failed to save: {:?}", e),
                    }
                }
            });
        });
        ui.separator();

        ui.horizontal(|ui| {
            ui.label("Max Simultaneous Conversions:");
            ui.add(
                egui::DragValue::new(&mut self.settings.maximum_number_of_simultaneous_conversions)
                    .range(1..=32),
            );

            ui.checkbox(
                &mut self.settings.copy_files_in_clipboard_after_conversion,
                "Copy output files to Clipboard",
            );
        });

        ui.separator();

        ui.columns(2, |columns| {
            // Left Column: Presets List
            columns[0].vertical(|ui| {
                ui.heading("Conversion Presets");
                ui.separator();

                for i in 0..self.settings.conversion_presets.len() {
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

                ui.separator();
                if ui.button("➕ Add Preset").clicked() {
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
            });

            // Right Column: Active Preset Configuration
            columns[1].vertical(|ui| {
                if self.selected_preset_index < self.settings.conversion_presets.len() {
                    let preset = &mut self.settings.conversion_presets[self.selected_preset_index];

                    ui.heading(format!("Edit Preset: {}", preset.name));
                    ui.separator();

                    ui.horizontal(|ui| {
                        ui.label("Preset Name:");
                        ui.text_edit_singleline(&mut preset.name);
                    });

                    ui.horizontal(|ui| {
                        ui.label("Output Format:");
                        ui.label(format!("{:?}", preset.output_type));
                    });

                    ui.horizontal(|ui| {
                        ui.label("Output File Template:");
                        ui.text_edit_singleline(&mut preset.output_file_name_template);
                    });

                    ui.horizontal(|ui| {
                        ui.label("Input Extensions (comma separated):");
                        let mut input_str = preset.input_types.join(", ");
                        if ui.text_edit_singleline(&mut input_str).changed() {
                            preset.input_types = input_str
                                .split(',')
                                .map(|s| s.trim().to_string())
                                .filter(|s| !s.is_empty())
                                .collect();
                        }
                    });
                } else {
                    ui.label("Select or create a preset on the left panel to configure.");
                }
            });
        });

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
            .with_inner_size([850.0, 600.0])
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

fn run_cli_conversions(args: Vec<String>) {
    let settings = match initialize_user_settings_if_needed() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Error initializing settings: {}", e);
            return;
        }
    };

    let mut preset_name = String::new();
    let mut input_files = Vec::new();

    let mut i = 1;
    while i < args.len() {
        if (args[i] == "-preset" || args[i] == "/preset") && i + 1 < args.len() {
            preset_name = args[i + 1].clone();
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

    let mut jobs = Vec::new();
    for (idx, file) in input_files.into_iter().enumerate() {
        jobs.push(ConversionJob::new(idx + 1, preset.clone(), file));
    }

    let max_threads = settings.maximum_number_of_simultaneous_conversions;
    let hw_accel = settings.hardware_acceleration_mode;
    let copy_clipboard = settings.copy_files_in_clipboard_after_conversion;

    let scheduler = ConversionScheduler::new(jobs, max_threads, hw_accel, copy_clipboard);

    println!("Starting conversion of {} files...", scheduler.jobs.len());
    scheduler.execute_all();
    println!("All conversion tasks finished.");
}
