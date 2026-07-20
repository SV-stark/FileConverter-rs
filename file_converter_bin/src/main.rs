#![allow(clippy::all, warnings)]
use std::env;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use file_converter_core::scheduler::{ConversionJob, ConversionScheduler, JobStatus};
use file_converter_core::settings::Settings;

// Embedded html dashboard
const HTML_CONTENT: &str = include_str!("index.html");

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
            // Write a basic default configuration XML string directly if default_xml is missing
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

    // Check if we need to open the GUI Settings dashboard
    let run_gui = args.len() < 2
        || args
            .iter()
            .any(|arg| arg == "-settings" || arg == "/settings");

    if run_gui {
        run_settings_web_gui();
    } else {
        run_cli_conversions(args);
    }
}

// GUI Server
fn run_settings_web_gui() {
    println!("Starting File Converter settings dashboard...");

    let settings = match initialize_user_settings_if_needed() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Error initializing settings: {}", e);
            return;
        }
    };

    // Find an open port
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let port = match server.server_addr() {
        tiny_http::ListenAddr::IP(addr) => addr.port(),
    };
    let url = format!("http://localhost:{}/", port);

    println!("Web server running on {}", url);
    let _ = webbrowser::open(&url);

    let settings_arc = Arc::new(Mutex::new(settings));

    // Request loop
    for request in server.incoming_requests() {
        let path = request.url().to_string();
        let method = request.method().clone();

        match (method, path.as_str()) {
            (tiny_http::Method::Get, "/") => {
                let response = tiny_http::Response::from_string(HTML_CONTENT).with_header(
                    tiny_http::Header::from_bytes(
                        &b"Content-Type"[..],
                        &b"text/html; charset=utf-8"[..],
                    )
                    .unwrap(),
                );
                let _ = request.respond(response);
            }
            (tiny_http::Method::Get, "/api/settings") => {
                let current = settings_arc.lock().unwrap();
                // Map the Rust Settings struct keys to C# PascalCase JSON representation
                // to make frontend JavaScript integration clean.
                // Or let serde_json do it:
                let json_data = serde_json::to_string(&*current).unwrap();
                let response = tiny_http::Response::from_string(json_data).with_header(
                    tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..])
                        .unwrap(),
                );
                let _ = request.respond(response);
            }
            (tiny_http::Method::Post, "/api/settings") => {
                let mut content = String::new();
                let mut req = request;
                let _ = req.as_reader().read_to_string(&mut content);

                match serde_json::from_str::<Settings>(&content) {
                    Ok(new_settings) => {
                        let (_, user_xml) = get_settings_paths();
                        if let Err(e) = new_settings.save_to_file(&user_xml) {
                            let response = tiny_http::Response::from_string(format!(
                                "Failed to save XML: {:?}",
                                e
                            ))
                            .with_status_code(500);
                            let _ = req.respond(response);
                        } else {
                            let mut current = settings_arc.lock().unwrap();
                            *current = new_settings;
                            let response = tiny_http::Response::from_string("Saved successfully");
                            let _ = req.respond(response);
                        }
                    }
                    Err(e) => {
                        let response =
                            tiny_http::Response::from_string(format!("Invalid JSON: {:?}", e))
                                .with_status_code(400);
                        let _ = req.respond(response);
                    }
                }
            }
            _ => {
                let response = tiny_http::Response::from_string("Not Found").with_status_code(404);
                let _ = request.respond(response);
            }
        }
    }
}

// CLI Processor
fn run_cli_conversions(args: Vec<String>) {
    let mut preset_name = String::new();
    let mut files = Vec::new();

    let mut i = 1;
    while i < args.len() {
        if args[i] == "-preset" && i + 1 < args.len() {
            preset_name = args[i + 1].clone();
            i += 2;
        } else {
            // Treat as input file
            files.push(args[i].clone());
            i += 1;
        }
    }

    if preset_name.is_empty() {
        eprintln!(
            "Error: No preset specified. Use -preset \"Preset Name\" followed by file paths."
        );
        return;
    }

    if files.is_empty() {
        eprintln!("Error: No input files specified.");
        return;
    }

    let settings = match initialize_user_settings_if_needed() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Error loading settings: {}", e);
            return;
        }
    };

    let preset = match settings.get_preset_from_name(&preset_name) {
        Some(p) => p.clone(),
        None => {
            eprintln!(
                "Error: Preset \"{}\" not found in configuration.",
                preset_name
            );
            return;
        }
    };

    println!("Loaded preset \"{}\"", preset.name);

    // Prepare jobs
    let mut jobs = Vec::new();
    for (idx, file) in files.iter().enumerate() {
        let mut job = ConversionJob::new(idx, preset.clone(), file.clone());
        if let Err(e) = job.prepare(idx, files.len()) {
            eprintln!("Skipping \"{}\" - preparation failed: {}", file, e);
            continue;
        }
        jobs.push(job);
    }

    if jobs.is_empty() {
        eprintln!("No jobs could be initialized.");
        return;
    }

    // Keep list of jobs
    let mut active_jobs = Vec::new();
    for j in &jobs {
        active_jobs.push((j.input_path.clone(), j.progress.clone(), j.status.clone()));
    }

    // Spawn status printing thread
    let print_handle = thread::spawn(move || {
        let term_clear_line = "\x1B[2K\r";
        let is_terminal = atty::is(atty::Stream::Stdout);

        loop {
            let mut all_done = true;
            let mut output = String::new();

            if is_terminal {
                // Move cursor to top of the block
                let lines_to_move = active_jobs.len();
                output.push_str(&format!("\x1B[{}A", lines_to_move));
            }

            for (path, progress, status) in &active_jobs {
                let p = *progress.lock().unwrap();
                let s = status.lock().unwrap().clone();

                let bar_width = 20;
                let filled = (p * bar_width as f32) as usize;
                let empty = bar_width - filled;
                let bar = format!(
                    "[{}{}] {:.0}%",
                    "=".repeat(filled),
                    " ".repeat(empty),
                    p * 100.0
                );

                let file_name = Path::new(path)
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or(path);

                let status_text = match &s {
                    JobStatus::Queue => "Queue".to_string(),
                    JobStatus::Converting(msg) => format!("Converting: {}", msg),
                    JobStatus::Done => "Done".to_string(),
                    JobStatus::Failed(e) => format!("Failed: {}", e),
                    JobStatus::Canceled => "Canceled".to_string(),
                };

                if !matches!(
                    &s,
                    JobStatus::Done | JobStatus::Failed(_) | JobStatus::Canceled
                ) {
                    all_done = false;
                }

                if is_terminal {
                    output.push_str(&format!(
                        "{}{:<25} {} - {}\n",
                        term_clear_line, file_name, bar, status_text
                    ));
                } else {
                    println!("{:<25} {} - {}", file_name, bar, status_text);
                }
            }

            if is_terminal {
                print!("{}", output);
                let _ = std::io::Write::flush(&mut std::io::stdout());
            }

            if all_done {
                break;
            }
            thread::sleep(Duration::from_millis(200));
        }
    });

    // Print empty lines first to reserve space for status updates
    if atty::is(atty::Stream::Stdout) {
        for _ in 0..jobs.len() {
            println!();
        }
    }

    // Run scheduler
    let scheduler = ConversionScheduler::new(
        jobs,
        settings.maximum_number_of_simultaneous_conversions,
        settings.hardware_acceleration_mode,
        settings.copy_files_in_clipboard_after_conversion,
    );

    scheduler.execute_all();

    // Wait for output thread to finish printing final state
    let _ = print_handle.join();

    println!("Conversion completed!");
}

// Simple atty module to detect if stdout is a terminal
mod atty {
    pub enum Stream {
        Stdout,
    }
    #[cfg(target_os = "windows")]
    pub fn is(_stream: Stream) -> bool {
        // Query Win32 GetFileType or standard isatty
        // To be extremely lightweight and robust on windows,
        // we check if stdout handle is character device type
        unsafe {
            use super::windows_sys::{GetFileType, GetStdHandle};
            const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;
            const FILE_TYPE_CHAR: u32 = 0x0002;
            let handle = GetStdHandle(STD_OUTPUT_HANDLE);
            GetFileType(handle) == FILE_TYPE_CHAR
        }
    }
    #[cfg(not(target_os = "windows"))]
    pub fn is(_stream: Stream) -> bool {
        false
    }
}

#[cfg(target_os = "windows")]
mod windows_sys {
    #[link(name = "kernel32")]
    extern "system" {
        pub fn GetStdHandle(nStdHandle: u32) -> *mut std::ffi::c_void;
        pub fn GetFileType(hFile: *mut std::ffi::c_void) -> u32;
    }
}
