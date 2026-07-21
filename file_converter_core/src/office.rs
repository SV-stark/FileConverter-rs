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

pub fn convert_office_to_pdf(app: &str, input_path: &str, output_path: &str) -> Result<()> {
    let script = match app.to_lowercase().as_str() {
        "word" | "winword.exe" => {
            format!(
                "$word = New-Object -ComObject Word.Application; \
                 $word.Visible = $false; \
                 $doc = $word.Documents.Open('{}'); \
                 $doc.ExportAsFixedFormat('{}', 17, $false, 0, 0, 1, 1, 0, $true, $true, 1, $true); \
                 $doc.Close(0); \
                 $word.Quit();",
                input_path.replace('\'', "''"),
                output_path.replace('\'', "''")
            )
        }
        "excel" | "excel.exe" => {
            format!(
                "$excel = New-Object -ComObject Excel.Application; \
                 $excel.Visible = $false; \
                 $wb = $excel.Workbooks.Open('{}', [System.Type]::Missing, $true); \
                 $wb.ExportAsFixedFormat(0, '{}'); \
                 $wb.Close($false); \
                 $excel.Quit();",
                input_path.replace('\'', "''"),
                output_path.replace('\'', "''")
            )
        }
        "powerpoint" | "powerpnt.exe" => {
            format!(
                "$ppt = New-Object -ComObject PowerPoint.Application; \
                 $doc = $ppt.Presentations.Open('{}', $true, $true, $false); \
                 $doc.ExportAsFixedFormat('{}', 2); \
                 $doc.Close(); \
                 $ppt.Quit();",
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

    let output = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output()
        .map_err(|e| {
            FileConverterError::Office(format!("Failed to execute powershell: {:?}", e))
        })?;

    if !output.status.success() {
        return Err(FileConverterError::Office(format!(
            "Office conversion script failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }

    Ok(())
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
                     $doc.Close(0); ",
                    inp.replace('\'', "''"),
                    out.replace('\'', "''")
                ));
            }
            format!(
                "$word = New-Object -ComObject Word.Application; \
                 $word.Visible = $false; \
                 {} \
                 $word.Quit();",
                pair_code
            )
        }
        "excel" | "excel.exe" => {
            let mut pair_code = String::new();
            for (inp, out) in input_output_pairs {
                pair_code.push_str(&format!(
                    "$wb = $excel.Workbooks.Open('{}', [System.Type]::Missing, $true); \
                     $wb.ExportAsFixedFormat(0, '{}'); \
                     $wb.Close($false); ",
                    inp.replace('\'', "''"),
                    out.replace('\'', "''")
                ));
            }
            format!(
                "$excel = New-Object -ComObject Excel.Application; \
                 $excel.Visible = $false; \
                 {} \
                 $excel.Quit();",
                pair_code
            )
        }
        "powerpoint" | "powerpnt.exe" => {
            let mut pair_code = String::new();
            for (inp, out) in input_output_pairs {
                pair_code.push_str(&format!(
                    "$doc = $ppt.Presentations.Open('{}', $true, $true, $false); \
                     $doc.ExportAsFixedFormat('{}', 2); \
                     $doc.Close(); ",
                    inp.replace('\'', "''"),
                    out.replace('\'', "''")
                ));
            }
            format!(
                "$ppt = New-Object -ComObject PowerPoint.Application; \
                 {} \
                 $ppt.Quit();",
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

    let output = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output()
        .map_err(|e| {
            FileConverterError::Office(format!("Failed to execute powershell: {:?}", e))
        })?;

    if !output.status.success() {
        return Err(FileConverterError::Office(format!(
            "Office conversion script failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }

    Ok(())
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
        let temp_dir = std::env::temp_dir();
        let file_name = Path::new(input_path)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("temp");
        let temp_pdf = crate::path_helpers::generate_unique_path(
            temp_dir.join(format!("{}_temp.pdf", file_name)),
            &[],
        );
        let temp_pdf_str = temp_pdf.to_string_lossy().to_string();

        convert_office_to_pdf(app_name, input_path, &temp_pdf_str)?;

        // Convert intermediate PDF to images
        let conversion_res = image::run_image_conversion(
            preset,
            &temp_pdf_str,
            output_file_paths,
            progress_callback,
        );

        // Clean up
        let _ = std::fs::remove_file(temp_pdf);

        conversion_res?;
    }

    Ok(())
}
