use crate::error::{FileConverterError, Result};
use crate::image;
use crate::settings::ConversionPreset;
use std::path::Path;
use std::process::Command;

#[cfg(target_os = "windows")]
pub fn is_office_app_available(app_name: &str) -> bool {
    use winreg::RegKey;
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};

    let subkey = format!(
        "Software\\Microsoft\\Windows\\CurrentVersion\\App Paths\\{}",
        app_name
    );

    if let Ok(hkcu) = RegKey::predef(HKEY_CURRENT_USER).open_subkey(&subkey)
        && hkcu.get_value::<String, _>("").is_ok()
    {
        return true;
    }

    if let Ok(hklm) = RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey(&subkey)
        && hklm.get_value::<String, _>("").is_ok()
    {
        return true;
    }

    false
}

#[cfg(not(target_os = "windows"))]
pub fn is_office_app_available(_app_name: &str) -> bool {
    false
}

fn base64_encode(bytes: &[u8]) -> String {
    const CHARSET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut result = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0];
        let b1 = chunk.get(1).copied().unwrap_or(0);
        let b2 = chunk.get(2).copied().unwrap_or(0);

        result.push(CHARSET[(b0 >> 2) as usize] as char);
        result.push(CHARSET[(((b0 & 0x03) << 4) | (b1 >> 4)) as usize] as char);
        if chunk.len() > 1 {
            result.push(CHARSET[(((b1 & 0x0f) << 2) | (b2 >> 6)) as usize] as char);
        } else {
            result.push('=');
        }
        if chunk.len() > 2 {
            result.push(CHARSET[(b2 & 0x3f) as usize] as char);
        } else {
            result.push('=');
        }
    }
    result
}

fn encode_powershell_script(script: &str) -> String {
    let utf16_bytes: Vec<u8> = script
        .encode_utf16()
        .flat_map(|u| u.to_le_bytes())
        .collect();
    base64_encode(&utf16_bytes)
}

pub fn convert_office_to_pdf(app: &str, input_path: &str, output_path: &str) -> Result<()> {
    let script = match app.to_lowercase().as_str() {
        "word" | "winword.exe" => {
            format!(
                "$word = $null; $doc = $null; \
                 try {{ \
                     $word = New-Object -ComObject Word.Application; \
                     $word.Visible = $false; \
                     $word.DisplayAlerts = 0; \
                     $doc = $word.Documents.Open('{}'); \
                     $doc.ExportAsFixedFormat('{}', 17, $false, 0, 0, 1, 1, 0, $true, $true, 1, $true); \
                 }} finally {{ \
                     if ($doc) {{ $doc.Close(0); }} \
                     if ($word) {{ $word.Quit(); }} \
                     [System.GC]::Collect(); \
                     [System.GC]::WaitForPendingFinalizers(); \
                 }}",
                input_path.replace('\'', "''"),
                output_path.replace('\'', "''")
            )
        }
        "excel" | "excel.exe" => {
            format!(
                "$excel = $null; $wb = $null; \
                 try {{ \
                     $excel = New-Object -ComObject Excel.Application; \
                     $excel.Visible = $false; \
                     $excel.DisplayAlerts = $false; \
                     $wb = $excel.Workbooks.Open('{}', [System.Type]::Missing, $true); \
                     $wb.ExportAsFixedFormat(0, '{}'); \
                 }} finally {{ \
                     if ($wb) {{ $wb.Close($false); }} \
                     if ($excel) {{ $excel.Quit(); }} \
                     [System.GC]::Collect(); \
                     [System.GC]::WaitForPendingFinalizers(); \
                 }}",
                input_path.replace('\'', "''"),
                output_path.replace('\'', "''")
            )
        }
        "powerpoint" | "powerpnt.exe" => {
            format!(
                "$ppt = $null; $doc = $null; \
                 try {{ \
                     $ppt = New-Object -ComObject PowerPoint.Application; \
                     $doc = $ppt.Presentations.Open('{}', $true, $true, $false); \
                     $doc.ExportAsFixedFormat('{}', 2); \
                 }} finally {{ \
                     if ($doc) {{ $doc.Close(); }} \
                     if ($ppt) {{ $ppt.Quit(); }} \
                     [System.GC]::Collect(); \
                     [System.GC]::WaitForPendingFinalizers(); \
                 }}",
                input_path.replace('\'', "''"),
                output_path.replace('\'', "''")
            )
        }
        _ => {
            return Err(FileConverterError::Office(format!(
                "Unsupported office application: {}",
                app
            )));
        }
    };

    execute_powershell_with_timeout(&script, 300)
}

fn execute_powershell_with_timeout(script: &str, timeout_secs: u64) -> Result<()> {
    let encoded = encode_powershell_script(script);
    let mut child = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-EncodedCommand", &encoded])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| {
            FileConverterError::Office(format!("Failed to execute powershell: {:?}", e))
        })?;

    let stderr_pipe = child.stderr.take();
    let stderr_handle = std::thread::spawn(move || {
        let mut stderr_str = String::new();
        if let Some(mut pipe) = stderr_pipe {
            let _ = std::io::Read::read_to_string(&mut pipe, &mut stderr_str);
        }
        stderr_str
    });

    let start = std::time::Instant::now();
    let max_dur = std::time::Duration::from_secs(timeout_secs);

    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let stderr_str = stderr_handle.join().unwrap_or_default();
                if status.success() {
                    return Ok(());
                } else {
                    return Err(FileConverterError::Office(format!(
                        "Office conversion script failed (exit code {:?}): {}",
                        status.code(),
                        stderr_str
                    )));
                }
            }
            Ok(None) => {
                if start.elapsed() > max_dur {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(FileConverterError::Timeout(format!(
                        "Office conversion timed out after {}s",
                        timeout_secs
                    )));
                }
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(FileConverterError::Office(format!(
                    "Failed waiting for PowerShell process: {:?}",
                    e
                )));
            }
        }
    }
}

pub fn convert_office_batch_to_pdf(app: &str, input_output_pairs: &[(&str, &str)]) -> Result<()> {
    if input_output_pairs.is_empty() {
        return Ok(());
    }

    if input_output_pairs.len() == 1 {
        return convert_office_to_pdf(app, input_output_pairs[0].0, input_output_pairs[0].1);
    }

    let script = match app.to_lowercase().as_str() {
        "word" | "winword.exe" => {
            let mut pair_code = String::new();
            for (inp, out) in input_output_pairs {
                pair_code.push_str(&format!(
                    "$doc = $word.Documents.Open('{}'); \
                     $doc.ExportAsFixedFormat('{}', 17, $false, 0, 0, 1, 1, 0, $true, $true, 1, $true); \
                     $doc.Close(0); $doc = $null; ",
                    inp.replace('\'', "''"),
                    out.replace('\'', "''")
                ));
            }
            format!(
                "$word = $null; $doc = $null; \
                 try {{ \
                     $word = New-Object -ComObject Word.Application; \
                     $word.Visible = $false; \
                     $word.DisplayAlerts = 0; \
                     {} \
                 }} finally {{ \
                     if ($doc) {{ $doc.Close(0); }} \
                     if ($word) {{ $word.Quit(); }} \
                     [System.GC]::Collect(); \
                     [System.GC]::WaitForPendingFinalizers(); \
                 }}",
                pair_code
            )
        }
        "excel" | "excel.exe" => {
            let mut pair_code = String::new();
            for (inp, out) in input_output_pairs {
                pair_code.push_str(&format!(
                    "$wb = $excel.Workbooks.Open('{}', [System.Type]::Missing, $true); \
                     $wb.ExportAsFixedFormat(0, '{}'); \
                     $wb.Close($false); $wb = $null; ",
                    inp.replace('\'', "''"),
                    out.replace('\'', "''")
                ));
            }
            format!(
                "$excel = $null; $wb = $null; \
                 try {{ \
                     $excel = New-Object -ComObject Excel.Application; \
                     $excel.Visible = $false; \
                     $excel.DisplayAlerts = $false; \
                     {} \
                 }} finally {{ \
                     if ($wb) {{ $wb.Close($false); }} \
                     if ($excel) {{ $excel.Quit(); }} \
                     [System.GC]::Collect(); \
                     [System.GC]::WaitForPendingFinalizers(); \
                 }}",
                pair_code
            )
        }
        "powerpoint" | "powerpnt.exe" => {
            let mut pair_code = String::new();
            for (inp, out) in input_output_pairs {
                pair_code.push_str(&format!(
                    "$doc = $ppt.Presentations.Open('{}', $true, $true, $false); \
                     $doc.ExportAsFixedFormat('{}', 2); \
                     $doc.Close(); $doc = $null; ",
                    inp.replace('\'', "''"),
                    out.replace('\'', "''")
                ));
            }
            format!(
                "$ppt = $null; $doc = $null; \
                 try {{ \
                     $ppt = New-Object -ComObject PowerPoint.Application; \
                     {} \
                 }} finally {{ \
                     if ($doc) {{ $doc.Close(); }} \
                     if ($ppt) {{ $ppt.Quit(); }} \
                     [System.GC]::Collect(); \
                     [System.GC]::WaitForPendingFinalizers(); \
                 }}",
                pair_code
            )
        }
        _ => {
            return Err(FileConverterError::Office(format!(
                "Unsupported office application: {}",
                app
            )));
        }
    };

    execute_powershell_with_timeout(&script, 300)
}

pub fn run_office_conversion(
    preset: &ConversionPreset,
    app_name: &str,
    input_path: &str,
    output_file_paths: &[String],
    progress_callback: &(dyn Fn(f32, &str) + Sync),
) -> Result<()> {
    progress_callback(0.0, "Read document");

    if output_file_paths.is_empty() {
        return Err(FileConverterError::Invalid(
            "No output file paths specified".to_string(),
        ));
    }

    let is_pdf_output = preset.output_type == crate::types::OutputType::Pdf;

    if is_pdf_output {
        let output_pdf = &output_file_paths[0];
        convert_office_to_pdf(app_name, input_path, output_pdf)?;
        progress_callback(1.0, "Done");
    } else {
        // Export to intermediate PDF
        let temp_dir = tempfile::tempdir()?;
        let file_name = Path::new(input_path)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("temp");
        let temp_pdf = crate::path_helpers::generate_unique_path(
            temp_dir.path().join(format!("{}_temp.pdf", file_name)),
            &[],
        );
        let temp_pdf_str = temp_pdf.to_string_lossy().to_string();

        convert_office_to_pdf(app_name, input_path, &temp_pdf_str)?;

        // Determine actual page count of the generated intermediate PDF
        let page_count = image::get_pdf_page_count(&temp_pdf_str).unwrap_or(1);

        let target_paths = if output_file_paths.len() < page_count {
            let mut paths = Vec::with_capacity(page_count);
            for i in 0..page_count {
                let out_path = crate::path_helpers::generate_file_path_from_template(
                    input_path,
                    preset.output_type.extension(),
                    &preset.output_file_name_template,
                    i + 1,
                    page_count,
                );
                let _ = crate::path_helpers::create_folders(&out_path);
                let unique = crate::path_helpers::generate_unique_path(&out_path, &paths);
                paths.push(unique.to_string_lossy().to_string());
            }
            paths
        } else {
            output_file_paths.to_vec()
        };

        // Convert intermediate PDF to images across all pages
        let conversion_res =
            image::run_image_conversion(preset, &temp_pdf_str, &target_paths, progress_callback);

        // Clean up
        let _ = std::fs::remove_file(temp_pdf);

        conversion_res?;
    }

    Ok(())
}
