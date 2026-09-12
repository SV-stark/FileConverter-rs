use jiff::Zoned;
use regex::Regex;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

static RE_DRIVE_LETTER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[a-zA-Z]:\\").unwrap());
static RE_VALID_PATH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?i)(?:\\\\[^\\/:*?<>|\r\n]+\\|[a-zA-Z]:\\|(?:[^\\/:*?<>|\r\n]+[\\/])+)?(?:[^\\/:*?<>|\r\n]+[\\/])*[^\\/:*?<>|\r\n]+$").unwrap()
});
static RE_DATE_FMT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\(d:(?P<format>[^)]*)\)").unwrap());

pub fn is_path_drive_letter_valid(path: &str) -> bool {
    RE_DRIVE_LETTER.is_match(path)
}

pub fn get_path_drive_letter(path: &str) -> Option<String> {
    RE_DRIVE_LETTER.find(path).map(|m| m.as_str().to_string())
}

/// Locate an executable by scanning each directory in the `PATH` environment
/// variable (replaces the `which` crate with a lightweight, dependency-free
/// equivalent sufficient for our lookup needs).
pub fn find_in_path(exe_name: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(exe_name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
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
    if let Some(parent) = file_path.as_ref().parent()
        && !parent.as_os_str().is_empty()
        && !parent.exists()
        && fs::create_dir_all(parent).is_err()
    {
        return false;
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
        && let Ok(val) = key.get_value::<String, _>(name)
    {
        return Some(val);
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

    // Path without extension - safe against multi-byte UTF-8
    let input_path_without_ext = if let Some(dot_idx) = input_file_path.rfind('.') {
        let last_slash = input_file_path.rfind(['/', '\\']).unwrap_or(0);
        if dot_idx > last_slash {
            &input_file_path[..dot_idx]
        } else {
            input_file_path
        }
    } else {
        input_file_path
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

    // Split directory folders without heap allocation using SmallVec and memchr
    let mut folders: smallvec::SmallVec<[&str; 16]> = smallvec::SmallVec::new();
    let mut start = 0;
    let bytes = parent_directory.as_bytes();
    while start < bytes.len() {
        if let Some(pos) = memchr::memchr2(b'/', b'\\', &bytes[start..]) {
            let seg = &parent_directory[start..start + pos];
            if !seg.is_empty() {
                folders.push(seg);
            }
            start += pos + 1;
        } else {
            let seg = &parent_directory[start..];
            if !seg.is_empty() {
                folders.push(seg);
            }
            break;
        }
    }

    let mut output_path = output_file_path_template.to_string();

    // Standard replacements
    if output_path.contains("(path)") {
        output_path = output_path.replace("(path)", &parent_dir_with_slash);
    }
    if output_path.contains("(p)") {
        output_path = output_path.replace("(p)", &parent_dir_with_slash);
    }

    if output_path.contains("(filename)") {
        output_path = output_path.replace("(filename)", file_name);
    }
    if output_path.contains("(f)") {
        output_path = output_path.replace("(f)", file_name);
    }
    if output_path.contains("(F)") {
        output_path = output_path.replace("(F)", &file_name.to_uppercase());
    }

    if output_path.contains("(outputext)") {
        output_path = output_path.replace("(outputext)", &output_extension);
    }
    if output_path.contains("(o)") {
        output_path = output_path.replace("(o)", &output_extension);
    }
    if output_path.contains("(O)") {
        output_path = output_path.replace("(O)", &output_extension.to_uppercase());
    }

    if output_path.contains("(inputext)") {
        output_path = output_path.replace("(inputext)", input_extension);
    }
    if output_path.contains("(i)") {
        output_path = output_path.replace("(i)", input_extension);
    }
    if output_path.contains("(I)") {
        output_path = output_path.replace("(I)", &input_extension.to_uppercase());
    }

    // Special folder paths - only query registry if template requests them
    if output_path.contains("(p:") {
        if output_path.contains("(p:d)") || output_path.contains("(p:documents)") {
            let doc_path = get_special_folder_path("documents");
            output_path = output_path.replace("(p:d)", &doc_path);
            output_path = output_path.replace("(p:documents)", &doc_path);
        }
        if output_path.contains("(p:m)") || output_path.contains("(p:music)") {
            let music_path = get_special_folder_path("music");
            output_path = output_path.replace("(p:m)", &music_path);
            output_path = output_path.replace("(p:music)", &music_path);
        }
        if output_path.contains("(p:v)") || output_path.contains("(p:videos)") {
            let video_path = get_special_folder_path("videos");
            output_path = output_path.replace("(p:v)", &video_path);
            output_path = output_path.replace("(p:videos)", &video_path);
        }
        if output_path.contains("(p:p)") || output_path.contains("(p:pictures)") {
            let pic_path = get_special_folder_path("pictures");
            output_path = output_path.replace("(p:p)", &pic_path);
            output_path = output_path.replace("(p:pictures)", &pic_path);
        }
    }

    // Directory nesting placeholders (d0), (d1), etc.
    if output_path.contains("(d") || output_path.contains("(D") {
        let folder_len = folders.len();
        for (i, &val) in folders.iter().enumerate().take(folder_len) {
            let d_index = folder_len - i - 1;
            let tag = format!("(d{})", d_index);
            if output_path.contains(&tag) {
                output_path = output_path.replace(&tag, val);
            }
            let upper_tag = format!("(D{})", d_index);
            if output_path.contains(&upper_tag) {
                output_path = output_path.replace(&upper_tag, &val.to_uppercase());
            }
        }
    }

    // Number index / count
    if output_path.contains("(n:i)") {
        output_path = output_path.replace("(n:i)", &number_index.to_string());
    }
    if output_path.contains("(n:c)") {
        output_path = output_path.replace("(n:c)", &number_max.to_string());
    }

    // Date formatting (d:format)
    if output_path.contains("(d:") {
        let now = Zoned::now();
        output_path = RE_DATE_FMT
            .replace_all(&output_path, |caps: &regex::Captures| {
                let fmt_str = translate_csharp_date_format(&caps["format"]);
                now.strftime(&fmt_str)
                    .to_string()
                    .replace('/', "-")
                    .replace(':', "'")
            })
            .to_string();
    }

    format!("{}.{}", output_path, output_extension)
}
