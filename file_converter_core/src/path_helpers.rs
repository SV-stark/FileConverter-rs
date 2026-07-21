use chrono::Local;
use regex::Regex;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

static RE_DRIVE_LETTER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[a-zA-Z]:\\").unwrap());
static RE_VALID_PATH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?i)(?:\\\\[^\\/:*?<>|\r\n]+\\|[a-zA-Z]:\\)(?:[^\\/:*?<>|\r\n]+\\)*[^\.\\/:*?<>|\r\n][^\\/:*?<>|\r\n]*$").unwrap()
});
static RE_DATE_FMT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\(d:(?P<format>[^)]*)\)").unwrap());

pub fn is_path_drive_letter_valid(path: &str) -> bool {
    RE_DRIVE_LETTER.is_match(path)
}

pub fn get_path_drive_letter(path: &str) -> Option<String> {
    RE_DRIVE_LETTER.find(path).map(|m| m.as_str().to_string())
}

pub fn is_path_valid(path: &str) -> bool {
    RE_VALID_PATH.is_match(path)
}

pub fn generate_unique_path<P: AsRef<Path>>(path: P, blacklist: &[String]) -> PathBuf {
    let path = path.as_ref();
    let mut unique_path = path.to_path_buf();

    let parent = path.parent().unwrap_or_else(|| Path::new(""));
    let file_stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let extension = path.extension().and_then(|s| s.to_str()).unwrap_or("");

    let ext_str = if extension.is_empty() {
        String::new()
    } else {
        format!(".{}", extension)
    };

    let mut index = 2;
    while unique_path.exists() || blacklist.iter().any(|b| Path::new(b) == unique_path) {
        let new_filename = format!("{} ({}){}", file_stem, index, ext_str);
        unique_path = parent.join(new_filename);
        index += 1;
    }

    unique_path
}

pub fn create_folders<P: AsRef<Path>>(file_path: P) -> bool {
    if let Some(parent) = file_path.as_ref().parent() {
        if !parent.exists() {
            if fs::create_dir_all(parent).is_err() {
                return false;
            }
        }
    }
    true
}

#[cfg(target_os = "windows")]
fn get_shell_folder(name: &str) -> Option<String> {
    use winreg::RegKey;
    use winreg::enums::HKEY_CURRENT_USER;

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    if let Ok(key) =
        hkcu.open_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\Shell Folders")
    {
        if let Ok(val) = key.get_value::<String, _>(name) {
            return Some(val);
        }
    }
    None
}

#[cfg(not(target_os = "windows"))]
fn get_shell_folder(_name: &str) -> Option<String> {
    None
}

pub fn get_special_folder_path(name: &str) -> String {
    let reg_name = match name {
        "documents" | "d" => "Personal",
        "music" | "m" => "My Music",
        "videos" | "v" => "My Video",
        "pictures" | "p" => "My Pictures",
        _ => "Personal",
    };

    if let Some(path) = get_shell_folder(reg_name) {
        let mut path = path;
        if !path.ends_with('\\') {
            path.push('\\');
        }
        path
    } else {
        // Fallback
        #[cfg(target_os = "windows")]
        {
            if let Ok(profile) = std::env::var("USERPROFILE") {
                let sub = match name {
                    "documents" | "d" => "Documents",
                    "music" | "m" => "Music",
                    "videos" | "v" => "Videos",
                    "pictures" | "p" => "Pictures",
                    _ => "Documents",
                };
                return format!("{}\\{}\\", profile, sub);
            }
        }
        String::from(".\\")
    }
}

fn translate_csharp_date_format(csharp_fmt: &str) -> String {
    csharp_fmt
        .replace("yyyy", "%Y")
        .replace("yy", "%y")
        .replace("MM", "%m")
        .replace("dd", "%d")
        .replace("HH", "%H")
        .replace("mm", "%M")
        .replace("ss", "%S")
}

pub fn generate_file_path_from_template(
    input_file_path: &str,
    output_extension: &str,
    output_file_path_template: &str,
    number_index: usize,
    number_max: usize,
) -> String {
    if input_file_path.is_empty() {
        return String::from("Invalid input file path.");
    }

    let path_buf = Path::new(input_file_path);
    let input_extension = path_buf.extension().and_then(|s| s.to_str()).unwrap_or("");

    // Path without extension
    let input_path_str = input_file_path;
    let input_path_without_ext = if !input_extension.is_empty() {
        &input_path_str[..input_path_str.len() - input_extension.len() - 1]
    } else {
        input_path_str
    };

    let output_extension = output_extension.to_lowercase();

    if output_file_path_template.is_empty() {
        return format!("{}.{}", input_path_without_ext, output_extension);
    }

    let file_name = path_buf.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let parent_directory = path_buf.parent().and_then(|p| p.to_str()).unwrap_or("");

    let mut parent_dir_with_slash = parent_directory.to_string();
    if !parent_dir_with_slash.is_empty()
        && !parent_dir_with_slash.ends_with('/')
        && !parent_dir_with_slash.ends_with('\\')
    {
        #[cfg(target_os = "windows")]
        parent_dir_with_slash.push('\\');
        #[cfg(not(target_os = "windows"))]
        parent_dir_with_slash.push('/');
    }

    // Split directory folders
    let folders: Vec<&str> = parent_directory
        .split(|c| c == '/' || c == '\\')
        .filter(|s| !s.is_empty())
        .collect();

    let mut output_path = output_file_path_template.to_string();

    // Standard replacements
    output_path = output_path.replace("(path)", &parent_dir_with_slash);
    output_path = output_path.replace("(p)", &parent_dir_with_slash);

    output_path = output_path.replace("(filename)", file_name);
    output_path = output_path.replace("(f)", file_name);
    output_path = output_path.replace("(F)", &file_name.to_uppercase());

    output_path = output_path.replace("(outputext)", &output_extension);
    output_path = output_path.replace("(o)", &output_extension);
    output_path = output_path.replace("(O)", &output_extension.to_uppercase());

    output_path = output_path.replace("(inputext)", input_extension);
    output_path = output_path.replace("(i)", input_extension);
    output_path = output_path.replace("(I)", &input_extension.to_uppercase());

    // Special folder paths
    output_path = output_path.replace("(p:d)", &get_special_folder_path("documents"));
    output_path = output_path.replace("(p:documents)", &get_special_folder_path("documents"));
    output_path = output_path.replace("(p:m)", &get_special_folder_path("music"));
    output_path = output_path.replace("(p:music)", &get_special_folder_path("music"));
    output_path = output_path.replace("(p:v)", &get_special_folder_path("videos"));
    output_path = output_path.replace("(p:videos)", &get_special_folder_path("videos"));
    output_path = output_path.replace("(p:p)", &get_special_folder_path("pictures"));
    output_path = output_path.replace("(p:pictures)", &get_special_folder_path("pictures"));

    // Directory nesting placeholders (d0), (d1), etc.
    let folder_len = folders.len();
    for i in 0..folder_len {
        let d_index = folder_len - i - 1;
        let val = folders[i];
        output_path = output_path.replace(&format!("(d{})", d_index), val);
        output_path = output_path.replace(&format!("(D{})", d_index), &val.to_uppercase());
    }

    // Number index / count
    output_path = output_path.replace("(n:i)", &number_index.to_string());
    output_path = output_path.replace("(n:c)", &number_max.to_string());

    // Date formatting (d:format)
    let now = Local::now();

    output_path = RE_DATE_FMT
        .replace_all(&output_path, |caps: &regex::Captures| {
            let fmt_str = translate_csharp_date_format(&caps["format"]);
            now.format(&fmt_str)
                .to_string()
                .replace('/', "-")
                .replace(':', "'")
        })
        .to_string();

    format!("{}.{}", output_path, output_extension)
}
