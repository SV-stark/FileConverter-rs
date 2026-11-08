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
use file_converter_core::types::HardwareAccelerationMode;

slint::include_modules!();
use slint::Model;

mod settings_logic;
mod settings_ui;

/// Title of the settings window. The drag-and-drop hook and the owner of
/// native dialogs find the window by it, so it must match `appwindow.slint`.
const SETTINGS_WINDOW_TITLE: &str = "File Converter Settings";

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

/// The single canonical copy of the default presets, embedded at build time.
///
/// It lives at the repository root (shared with `file_converter_shell`, the
/// installer and the release packaging). Do **not** add a crate-local copy: the
/// duplicates that used to exist here silently shipped stale presets.
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

    let mut settings = Settings::load_from_file(&user_xml)
        .map_err(|e| format!("Failed to load settings: {:?}", e))?;

    // Adopt presets that were added to the shipped defaults after this user's
    // settings file was created. Without this, preset fixes shipped in a new
    // version would never reach existing installations (the "Import Presets"
    // button used to be the only way, and it is easy to miss).
    if let Ok(defaults) = Settings::load_from_str(DEFAULT_SETTINGS_XML) {
        let before = settings.conversion_presets.len();
        settings.merge(defaults);
        if settings.conversion_presets.len() != before {
            let _ = settings.save_to_file(&user_xml);
        }
    }

    Ok(settings)
}

/// Runs `regsvr32.exe` (elevated) to register or unregister the shell
/// extension DLL. Both write to HKLM, so both need administrator rights:
/// unregistering used to run without elevation and silently do nothing.
fn register_shell_extension(unregister: bool) -> std::io::Result<()> {
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    use windows::core::PCWSTR;

    let mut exe_dir = env::current_exe().unwrap_or_default();
    exe_dir.pop();
    let dll_path = exe_dir.join("file_converter_shell.dll");

    if !dll_path.exists() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("Shell DLL not found at {}", dll_path.display()),
        ));
    }

    let switches = if unregister { "/u /s" } else { "/s" };
    let params: Vec<u16> = format!("{switches} \"{}\"\0", dll_path.display())
        .encode_utf16()
        .collect();
    let verb: Vec<u16> = "runas".encode_utf16().chain(std::iter::once(0)).collect();
    let file: Vec<u16> = "regsvr32.exe"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();

    // SAFETY: `ShellExecuteW` is a Win32 shim; all pointers reference
    // null-terminated buffers that live until the call returns, and no
    // caller-owned memory is aliased.
    let result = unsafe {
        ShellExecuteW(
            None,
            PCWSTR(verb.as_ptr()),
            PCWSTR(file.as_ptr()),
            PCWSTR(params.as_ptr()),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };

    if (result.0 as usize) > 32 {
        Ok(())
    } else {
        Err(std::io::Error::other(format!(
            "The request was canceled or failed (code {}).",
            result.0 as usize
        )))
    }
}

fn register_shell_extension_dll() -> String {
    match register_shell_extension(false) {
        Ok(()) => {
            "Adding File Converter to the Explorer menu. Accept the Windows prompt to finish."
                .to_string()
        }
        Err(e) => e.to_string(),
    }
}

fn unregister_shell_extension_dll() -> String {
    match register_shell_extension(true) {
        Ok(()) => {
            "Removing File Converter from the Explorer menu. Accept the Windows prompt to finish."
                .to_string()
        }
        Err(e) => e.to_string(),
    }
}

/// The settings window, used as the owner of native dialogs so they are
/// modal and centred on it (null when it isn't open).
#[cfg(target_os = "windows")]
fn owner_hwnd() -> windows::Win32::Foundation::HWND {
    find_our_window_hwnd().unwrap_or(windows::Win32::Foundation::HWND(std::ptr::null_mut()))
}

#[cfg(target_os = "windows")]
fn message_box(
    text: &str,
    style: windows::Win32::UI::WindowsAndMessaging::MESSAGEBOX_STYLE,
) -> windows::Win32::UI::WindowsAndMessaging::MESSAGEBOX_RESULT {
    use windows::Win32::UI::WindowsAndMessaging::MessageBoxW;
    use windows::core::PCWSTR;
    let text: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let title: Vec<u16> = "File Converter"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: both buffers are NUL-terminated and outlive the call.
    unsafe {
        MessageBoxW(
            owner_hwnd(),
            PCWSTR(text.as_ptr()),
            PCWSTR(title.as_ptr()),
            style,
        )
    }
}

/// Native yes/no question (confirming a delete, a discard...).
#[cfg(target_os = "windows")]
fn ask_yes_no(question: &str) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{IDYES, MB_ICONQUESTION, MB_YESNO};
    message_box(question, MB_YESNO | MB_ICONQUESTION) == IDYES
}

#[cfg(not(target_os = "windows"))]
fn ask_yes_no(_question: &str) -> bool {
    true
}

/// Native yes/no/cancel question: `Some(true)` yes, `Some(false)` no,
/// `None` cancel.
#[cfg(target_os = "windows")]
fn ask_yes_no_cancel(question: &str) -> Option<bool> {
    use windows::Win32::UI::WindowsAndMessaging::{IDNO, IDYES, MB_ICONWARNING, MB_YESNOCANCEL};
    match message_box(question, MB_YESNOCANCEL | MB_ICONWARNING) {
        r if r == IDYES => Some(true),
        r if r == IDNO => Some(false),
        _ => None,
    }
}

#[cfg(not(target_os = "windows"))]
fn ask_yes_no_cancel(_question: &str) -> Option<bool> {
    Some(false)
}

/// Opens a file, folder or URL with its default program.
fn open_path_with_shell(target: &str) {
    #[cfg(target_os = "windows")]
    {
        use windows::Win32::UI::Shell::ShellExecuteW;
        use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
        use windows::core::PCWSTR;
        let target: Vec<u16> = target.encode_utf16().chain(std::iter::once(0)).collect();
        let verb: Vec<u16> = "open".encode_utf16().chain(std::iter::once(0)).collect();
        // SAFETY: both buffers are NUL-terminated and outlive the call.
        unsafe {
            let _ = ShellExecuteW(
                owner_hwnd(),
                PCWSTR(verb.as_ptr()),
                PCWSTR(target.as_ptr()),
                PCWSTR::null(),
                PCWSTR::null(),
                SW_SHOWNORMAL,
            );
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = std::process::Command::new("xdg-open").arg(target).spawn();
    }
}

/// Opens a web page; anything that isn't http(s) is ignored.
fn open_url(url: &str) {
    if url.starts_with("http://") || url.starts_with("https://") {
        open_path_with_shell(url);
    }
}

/// Shows a file selected in Explorer, or its folder when the file is gone.
/// Returns false when neither exists.
fn reveal_in_explorer(path: &str) -> bool {
    let p = Path::new(path);
    if p.is_file() {
        let mut cmd = std::process::Command::new("explorer");
        #[cfg(target_os = "windows")]
        {
            // `explorer` wants `/select,"C:\a b\c.mp3"`; the default argument
            // quoting would wrap the whole thing and break paths with commas.
            use std::os::windows::process::CommandExt;
            cmd.raw_arg(format!("/select,\"{path}\""));
        }
        #[cfg(not(target_os = "windows"))]
        cmd.arg(path);
        let _ = cmd.spawn();
        return true;
    }
    match p.parent().filter(|d| d.is_dir()) {
        Some(dir) => {
            open_path_with_shell(&dir.to_string_lossy());
            true
        }
        None => false,
    }
}

/// Writes only the theme choice to the settings file, leaving everything
/// else as it is on disk. The settings window keeps unsaved edits in memory
/// and must not have them written behind the user's back.
fn persist_theme_preference(dark: Option<bool>) {
    let path = get_settings_paths().1;
    match Settings::load_from_file(&path) {
        Ok(mut s) => {
            s.dark_mode = dark;
            if let Err(e) = s.save_to_file(&path) {
                tracing::warn!("Failed to save the theme preference: {e:?}");
            }
        }
        Err(e) => tracing::warn!("Failed to load settings to save the theme: {e:?}"),
    }
}

fn play_completion_sound() {
    use windows::Win32::System::Diagnostics::Debug::MessageBeep;
    use windows::Win32::UI::WindowsAndMessaging::MESSAGEBOX_STYLE;

    // SAFETY: `MessageBeep` only plays a system notification sound and takes no
    // pointers or caller-owned memory.
    unsafe {
        let _ = MessageBeep(MESSAGEBOX_STYLE(0x00000040));
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

/// Most recent history entries kept on disk.
const HISTORY_LIMIT: usize = 200;

/// Prepends a whole batch to the history in one read + one write.
///
/// This used to load, rewrite and save `history.json` once *per file*, so
/// finishing a 1,000-file batch meant 1,000 full rewrites on the UI thread.
fn add_history_records(records: Vec<HistoryRecord>) {
    if records.is_empty() {
        return;
    }
    let mut history = load_history();
    let mut combined = records;
    combined.append(&mut history);
    combined.truncate(HISTORY_LIMIT);
    save_history(&combined);
}

fn history_timestamp() -> String {
    jiff::Zoned::now().strftime("%Y-%m-%d %H:%M:%S").to_string()
}

/// Formats a duration as `m:ss` (or `h:mm:ss`).
fn format_duration(d: Duration) -> String {
    let secs = d.as_secs();
    if secs >= 3600 {
        format!("{}:{:02}:{:02}", secs / 3600, (secs % 3600) / 60, secs % 60)
    } else {
        format!("{}:{:02}", secs / 60, secs % 60)
    }
}

/// Shows a native error dialog. The binary is a GUI-subsystem program, so
/// errors that were only logged (unknown preset, unreadable settings) used to
/// look like "nothing happened" when launched from Explorer.
fn show_error_box(message: &str) {
    #[cfg(target_os = "windows")]
    {
        use windows::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};
        use windows::core::PCWSTR;
        let text: Vec<u16> = message.encode_utf16().chain(std::iter::once(0)).collect();
        let title: Vec<u16> = "File Converter"
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        // SAFETY: both buffers are NUL-terminated and outlive the call.
        unsafe {
            MessageBoxW(
                None,
                PCWSTR(text.as_ptr()),
                PCWSTR(title.as_ptr()),
                MB_OK | MB_ICONERROR,
            );
        }
    }
    eprintln!("{message}");
}

/// Lets `println!` reach the terminal when the GUI-subsystem binary is started
/// from a console (`list-presets`, `convert --headless`, `--help`). Without this
/// every CLI command printed into the void.
fn attach_parent_console() {
    #[cfg(target_os = "windows")]
    {
        use windows::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};
        // SAFETY: plain Win32 call without pointers; failure (no parent console,
        // e.g. launched from Explorer) is harmless and ignored.
        unsafe {
            let _ = AttachConsole(ATTACH_PARENT_PROCESS);
        }
    }
}

/// Returns `base`, or `base (2)`, `base (3)`... so preset names stay unique
/// (the context menu and the CLI address presets by name).
fn unique_preset_name(settings: &Settings, base: &str) -> String {
    if !settings.conversion_presets.iter().any(|p| p.name == base) {
        return base.to_string();
    }
    (2..)
        .map(|n| format!("{base} ({n})"))
        .find(|candidate| {
            !settings
                .conversion_presets
                .iter()
                .any(|p| &p.name == candidate)
        })
        .unwrap_or_else(|| base.to_string())
}

fn is_windows_dark_mode() -> bool {
    #[cfg(target_os = "windows")]
    {
        use winreg::RegKey;
        use winreg::enums::HKEY_CURRENT_USER;
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        if let Ok(key) =
            hkcu.open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize")
        {
            if let Ok(val) = key.get_value::<u32, _>("AppsUseLightTheme") {
                return val == 0;
            }
        }
    }
    false
}

fn is_preset_compatible_with_file(
    preset: &file_converter_core::settings::ConversionPreset,
    file_path: &str,
) -> bool {
    let declared: Vec<String> = preset.input_types.iter().map(|s| s.to_string()).collect();
    file_converter_core::types::is_preset_applicable_to_file(
        preset.output_type,
        &declared,
        file_path,
    )
}

#[cfg(target_os = "windows")]
fn open_file_dialog() -> Vec<String> {
    use windows::Win32::System::Com::{
        CLSCTX_ALL, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
    };
    use windows::Win32::UI::Shell::{
        FOS_ALLOWMULTISELECT, FOS_FILEMUSTEXIST, FileOpenDialog, IFileOpenDialog, SIGDN_FILESYSPATH,
    };

    // SAFETY: COM is initialized on the UI thread for FileOpenDialog and cleaned up via scopeguard.
    unsafe {
        let coinit = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let _guard = scopeguard::guard((), |_| {
            if coinit.is_ok() {
                windows::Win32::System::Com::CoUninitialize();
            }
        });
        let dialog_res: windows::core::Result<IFileOpenDialog> =
            CoCreateInstance(&FileOpenDialog, None, CLSCTX_ALL);
        if let Ok(dialog) = dialog_res {
            let _ = dialog.SetOptions(FOS_ALLOWMULTISELECT | FOS_FILEMUSTEXIST);
            let _ = dialog.SetTitle(windows::core::w!("Select Files to Convert"));
            if dialog.Show(owner_hwnd()).is_ok() {
                if let Ok(results) = dialog.GetResults() {
                    let mut file_paths = Vec::new();
                    if let Ok(count) = results.GetCount() {
                        for i in 0..count {
                            if let Ok(item) = results.GetItemAt(i) {
                                if let Ok(display_name) = item.GetDisplayName(SIGDN_FILESYSPATH) {
                                    if !display_name.is_null() {
                                        let path_str = display_name.to_string().unwrap_or_default();
                                        CoTaskMemFree(Some(display_name.0 as *const _));
                                        if !path_str.is_empty() {
                                            file_paths.push(path_str);
                                        }
                                    }
                                }
                            }
                        }
                    }
                    return file_paths;
                }
            }
        }
    }
    Vec::new()
}

#[cfg(not(target_os = "windows"))]
fn open_file_dialog() -> Vec<String> {
    Vec::new()
}

/// Native "open preset file" / "save preset file" dialogs (XML filter).
#[cfg(target_os = "windows")]
fn preset_file_dialog(save: bool, default_name: &str) -> Option<String> {
    use windows::Win32::System::Com::{
        CLSCTX_ALL, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
    };
    use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
    use windows::Win32::UI::Shell::{
        FOS_FILEMUSTEXIST, FOS_OVERWRITEPROMPT, FileOpenDialog, FileSaveDialog, IFileDialog,
        IFileOpenDialog, IFileSaveDialog, SIGDN_FILESYSPATH,
    };
    use windows::core::{Interface, PCWSTR, w};

    // SAFETY: COM is initialised on this (UI) thread for the dialog's lifetime;
    // every string passed in is a NUL-terminated buffer that outlives the call.
    unsafe {
        let coinit = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let _guard = scopeguard::guard((), |_| {
            if coinit.is_ok() {
                windows::Win32::System::Com::CoUninitialize();
            }
        });

        let dialog: IFileDialog = if save {
            let d: IFileSaveDialog = CoCreateInstance(&FileSaveDialog, None, CLSCTX_ALL).ok()?;
            d.cast().ok()?
        } else {
            let d: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_ALL).ok()?;
            d.cast().ok()?
        };

        let filters = [
            COMDLG_FILTERSPEC {
                pszName: w!("Preset files (*.xml)"),
                pszSpec: w!("*.xml"),
            },
            COMDLG_FILTERSPEC {
                pszName: w!("All files (*.*)"),
                pszSpec: w!("*.*"),
            },
        ];
        let _ = dialog.SetFileTypes(&filters);
        let _ = dialog.SetDefaultExtension(w!("xml"));
        if save {
            let _ = dialog.SetOptions(FOS_OVERWRITEPROMPT);
            let _ = dialog.SetTitle(w!("Export presets"));
            let name: Vec<u16> = default_name
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect();
            let _ = dialog.SetFileName(PCWSTR(name.as_ptr()));
        } else {
            let _ = dialog.SetOptions(FOS_FILEMUSTEXIST);
            let _ = dialog.SetTitle(w!("Import presets"));
        }

        dialog.Show(owner_hwnd()).ok()?;
        let item = dialog.GetResult().ok()?;
        let name = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let path = name.to_string().ok();
        CoTaskMemFree(Some(name.0 as *const _));
        path.filter(|p| !p.is_empty())
    }
}

#[cfg(not(target_os = "windows"))]
fn preset_file_dialog(_save: bool, _default_name: &str) -> Option<String> {
    None
}

/// `EnumWindows` callback: stops at the first top-level window of *this*
/// process whose title is the settings window's.
#[cfg(target_os = "windows")]
unsafe extern "system" fn enum_windows_proc(
    hwnd: windows::Win32::Foundation::HWND,
    lparam: windows::Win32::Foundation::LPARAM,
) -> windows::Win32::Foundation::BOOL {
    use windows::Win32::UI::WindowsAndMessaging::{GetWindowTextW, GetWindowThreadProcessId};
    let mut pid = 0u32;
    // SAFETY: `lparam` carries the `*mut HWND` passed by `find_our_window_hwnd`,
    // which outlives the enumeration; the text buffer is a local array.
    unsafe {
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid != std::process::id() {
            return windows::Win32::Foundation::BOOL(1);
        }
        let mut buf = [0u16; 128];
        let len = GetWindowTextW(hwnd, &mut buf).max(0) as usize;
        if String::from_utf16_lossy(&buf[..len]) == SETTINGS_WINDOW_TITLE {
            let target = lparam.0 as *mut windows::Win32::Foundation::HWND;
            *target = hwnd;
            return windows::Win32::Foundation::BOOL(0);
        }
    }
    windows::Win32::Foundation::BOOL(1)
}

/// The settings window of this process. Matching on the process *and* the
/// title matters: the old fallback took the process's first top-level window,
/// which can be a hidden helper window, so file drops never arrived.
#[cfg(target_os = "windows")]
fn find_our_window_hwnd() -> Option<windows::Win32::Foundation::HWND> {
    use windows::Win32::UI::WindowsAndMessaging::EnumWindows;
    let mut target = windows::Win32::Foundation::HWND(std::ptr::null_mut());
    // SAFETY: the callback only writes through the pointer to `target`, which
    // lives until `EnumWindows` returns.
    unsafe {
        let _ = EnumWindows(
            Some(enum_windows_proc),
            windows::Win32::Foundation::LPARAM(&mut target as *mut _ as isize),
        );
    }
    (!target.0.is_null()).then_some(target)
}

#[cfg(target_os = "windows")]
type NativeDropCallback = Box<dyn Fn(Vec<String>) + Send + Sync + 'static>;

#[cfg(target_os = "windows")]
static DROP_CALLBACK: std::sync::Mutex<Option<NativeDropCallback>> = std::sync::Mutex::new(None);

#[cfg(target_os = "windows")]
#[link(name = "comctl32")]
unsafe extern "system" {
    fn SetWindowSubclass(
        hwnd: windows::Win32::Foundation::HWND,
        pfnSubclass: Option<
            unsafe extern "system" fn(
                windows::Win32::Foundation::HWND,
                u32,
                windows::Win32::Foundation::WPARAM,
                windows::Win32::Foundation::LPARAM,
                usize,
                usize,
            ) -> windows::Win32::Foundation::LRESULT,
        >,
        uIdSubclass: usize,
        dwRefData: usize,
    ) -> windows::Win32::Foundation::BOOL;

    fn DefSubclassProc(
        hwnd: windows::Win32::Foundation::HWND,
        uMsg: u32,
        wParam: windows::Win32::Foundation::WPARAM,
        lParam: windows::Win32::Foundation::LPARAM,
    ) -> windows::Win32::Foundation::LRESULT;
}

#[cfg(target_os = "windows")]
unsafe extern "system" fn drop_subclass_proc(
    hwnd: windows::Win32::Foundation::HWND,
    msg: u32,
    wparam: windows::Win32::Foundation::WPARAM,
    lparam: windows::Win32::Foundation::LPARAM,
    _uidsubclass: usize,
    _refdata: usize,
) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::UI::Shell::{DragFinish, DragQueryFileW, HDROP};
    use windows::Win32::UI::WindowsAndMessaging::WM_DROPFILES;

    if msg == WM_DROPFILES {
        let hdrop = HDROP(wparam.0 as *mut _);
        let mut files = Vec::new();
        unsafe {
            let count = DragQueryFileW(hdrop, 0xFFFFFFFF, None);
            for i in 0..count {
                let len = DragQueryFileW(hdrop, i, None);
                if len > 0 {
                    let mut buf = vec![0u16; (len + 1) as usize];
                    DragQueryFileW(hdrop, i, Some(&mut buf));
                    if let Some(pos) = buf.iter().position(|&c| c == 0) {
                        files.push(String::from_utf16_lossy(&buf[..pos]));
                    }
                }
            }
            DragFinish(hdrop);
        }

        if !files.is_empty() {
            if let Ok(guard) = DROP_CALLBACK.lock() {
                if let Some(ref cb) = *guard {
                    cb(files);
                }
            }
        }
        return windows::Win32::Foundation::LRESULT(0);
    }

    unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
}

#[cfg(target_os = "windows")]
fn setup_window_drag_and_drop<F>(on_drop: F)
where
    F: Fn(Vec<String>) + Send + Sync + 'static,
{
    if let Ok(mut guard) = DROP_CALLBACK.lock() {
        *guard = Some(Box::new(on_drop));
    }

    std::thread::spawn(|| {
        for _ in 0..40 {
            std::thread::sleep(Duration::from_millis(50));
            if let Some(hwnd) = find_our_window_hwnd() {
                use windows::Win32::UI::Shell::DragAcceptFiles;
                unsafe {
                    DragAcceptFiles(hwnd, true);
                    let _ = SetWindowSubclass(hwnd, Some(drop_subclass_proc), 1001, 0);
                }
                break;
            }
        }
    });
}

use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(name = "file_converter_bin")]
#[command(
    author = "File Converter Team",
    // Derived from the crate version so `--version` can never drift again.
    version = env!("CARGO_PKG_VERSION"),
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
    let mut jobs = Vec::with_capacity(total_input_files);
    for (idx, file) in input_files.iter().enumerate() {
        let mut job = ConversionJob::new(idx + 1, preset.clone(), file.clone());
        if let Err(e) = job.prepare(idx, total_input_files) {
            // `prepare` records the reason, so the job reports it verbatim
            // instead of failing later with "no output path specified".
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
            eprintln!(
                "Preset '{}' not found. Run `file_converter_bin list-presets` to see the available names.",
                preset_name
            );
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
    let mut history = Vec::with_capacity(scheduler.jobs.len());
    for job in &scheduler.jobs {
        let status = job.status.lock().clone();
        let outputs = job.output_file_paths.join("; ");
        match &status {
            JobStatus::Done => println!("[OK] {} -> {}", job.input_path, outputs),
            JobStatus::Failed(e) => {
                eprintln!("[FAILED] {}: {}", job.input_path, e);
                failed += 1;
            }
            _ => {}
        }
        history.push(HistoryRecord {
            timestamp: history_timestamp(),
            preset_name: preset.name.clone(),
            input_path: job.input_path.clone(),
            output_path: outputs,
            status: match status {
                JobStatus::Done => "Done".to_string(),
                JobStatus::Failed(e) => format!("Failed ({e})"),
                JobStatus::Canceled => "Canceled".to_string(),
                _ => "Finished".to_string(),
            },
        });
    }
    add_history_records(history);
    println!(
        "{} converted, {} failed.",
        scheduler.jobs.len() - failed,
        failed
    );

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
    let raw_args: Vec<String> = env::args().collect();
    if raw_args.len() > 1 {
        attach_parent_console();
    }
    tracing_subscriber::fmt::init();

    let normalized = normalize_args(&raw_args);

    let cli = match Cli::try_parse_from(&normalized) {
        Ok(c) => c,
        Err(err) => {
            // `--help`, `--version` and usage errors must be printed, not
            // swallowed into a GUI launch (the binary is `windows_subsystem =
            // "windows"`, so a silent launch looks like a hang to the user).
            use clap::error::ErrorKind;

            match err.kind() {
                ErrorKind::DisplayHelp
                | ErrorKind::DisplayVersion
                | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand => {
                    print!("{}", err);
                    return;
                }
                ErrorKind::InvalidSubcommand
                | ErrorKind::UnknownArgument
                | ErrorKind::MissingRequiredArgument
                | ErrorKind::InvalidValue => {
                    eprintln!("{}", err.render().ansi());
                    eprintln!(
                        "Run with no arguments to open the settings window, or pass file paths \
                         to convert."
                    );
                    return;
                }
                _ => {}
            }

            if raw_args.len() <= 1 {
                settings_ui::run(None);
            } else {
                let mut possible_files = Vec::new();
                for arg in raw_args.iter().skip(1) {
                    if Path::new(arg).exists() {
                        possible_files.push(arg.clone());
                    }
                }
                if !possible_files.is_empty() {
                    settings_ui::run(Some(possible_files));
                } else {
                    tracing::error!("Invalid command line arguments.");
                }
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
                settings_ui::run(None);
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
        settings_ui::run(None);
        return;
    }

    if let Some(preset_name) = cli.preset {
        let mut input_files = Vec::new();
        let mut temp_to_clean = None;

        if let Some(ref list_path) = cli.input_files {
            if list_path.exists() {
                if let Ok(content) = std::fs::read_to_string(list_path) {
                    for line in content.lines() {
                        // Only the line terminator may be stripped. `trim()` would
                        // also eat leading/trailing spaces, which are legal in
                        // Windows paths ("C:\My Files\ a.png") and would silently
                        // convert a different (usually non-existent) file.
                        let path = line.trim_end_matches(['\r', '\n']);
                        if !path.is_empty() {
                            input_files.push(path.to_string());
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
    } else if !cli.files.is_empty() {
        settings_ui::run(Some(cli.files));
    } else {
        settings_ui::run(None);
    }
}

fn spawn_conversion_process(preset_name: &str, files: &[String]) {
    let Ok(exe_path) = std::env::current_exe() else {
        return;
    };
    let mut cmd = std::process::Command::new(exe_path);
    cmd.arg("--preset").arg(preset_name);

    // Windows caps a command line at 32K characters; large drops are handed
    // over through a list file instead (the converter deletes it afterwards),
    // exactly like the Explorer context menu does.
    let total_len: usize = files.iter().map(|f| f.len() + 3).sum();
    let list_file = (total_len >= 8000)
        .then(|| {
            let path = std::env::temp_dir().join(format!(
                "file-converter-input-list-{}-{}.txt",
                std::process::id(),
                jiff::Timestamp::now().as_nanosecond()
            ));
            std::fs::write(&path, files.join("\n")).ok().map(|_| path)
        })
        .flatten();

    match list_file {
        Some(path) => {
            cmd.arg("--input-files").arg(path);
        }
        None => {
            cmd.args(files);
        }
    }
    let _ = cmd.spawn();
}

fn hw_accel_label(mode: HardwareAccelerationMode) -> &'static str {
    match mode {
        HardwareAccelerationMode::Auto => "Auto",
        HardwareAccelerationMode::Off => "Off",
        HardwareAccelerationMode::Cuda => "CUDA",
        HardwareAccelerationMode::Amf => "AMF",
        HardwareAccelerationMode::Qsv => "QSV",
    }
}

fn parse_hw_accel_label(label: &str) -> HardwareAccelerationMode {
    match label.to_ascii_uppercase().as_str() {
        "OFF" => HardwareAccelerationMode::Off,
        "CUDA" => HardwareAccelerationMode::Cuda,
        "AMF" => HardwareAccelerationMode::Amf,
        "QSV" => HardwareAccelerationMode::Qsv,
        _ => HardwareAccelerationMode::Auto,
    }
}

fn file_name_of(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string())
}

/// Builds the progress-window row for one job.
fn job_row(job: &ConversionJob) -> JobProgressData {
    let status = job.status.lock().clone();
    let outputs = || {
        let names: Vec<String> = job
            .output_file_paths
            .iter()
            .map(|p| file_name_of(p))
            .collect();
        match names.len() {
            0 => String::new(),
            1 => names[0].clone(),
            n => format!("{} … ({n} files)", names[0]),
        }
    };

    let (status_text, detail, is_done, is_failed) = match &status {
        JobStatus::Queue => (
            "Queued".to_string(),
            match &job.preparation_error {
                Some(err) => err.clone(),
                None => format!("→ {}", outputs()),
            },
            false,
            false,
        ),
        JobStatus::Converting(msg) => {
            let mut detail = msg.clone();
            if let Some(elapsed) = job.elapsed() {
                detail.push_str(&format!(" · {} elapsed", format_duration(elapsed)));
            }
            if let Some(left) = job.estimated_remaining() {
                detail.push_str(&format!(" · ~{} left", format_duration(left)));
            }
            ("Converting".to_string(), detail, false, false)
        }
        JobStatus::Done => {
            let mut detail = format!("Saved as {}", outputs());
            if let Some(elapsed) = job.elapsed() {
                detail.push_str(&format!(" · took {}", format_duration(elapsed)));
            }
            ("Done".to_string(), detail, true, false)
        }
        JobStatus::Failed(err) => ("Failed".to_string(), err.clone(), false, true),
        JobStatus::Canceled => (
            "Canceled".to_string(),
            "Canceled by user".to_string(),
            false,
            true,
        ),
    };

    JobProgressData {
        id: job.id as i32,
        input_file_name: file_name_of(&job.input_path).into(),
        input_path: job.input_path.as_str().into(),
        output_path: job.output_file_paths.join("; ").into(),
        progress: job.get_progress(),
        status_text: status_text.into(),
        detail: detail.into(),
        is_done,
        is_failed,
    }
}

fn run_conversion_gui(
    preset_name: &str,
    input_files: Vec<String>,
    temp_list_to_clean: Option<PathBuf>,
) {
    // The list file is ours to delete however this function exits.
    let _cleanup = scopeguard::guard(temp_list_to_clean, |path| {
        if let Some(path) = path {
            let _ = std::fs::remove_file(path);
        }
    });

    let settings = match initialize_user_settings_if_needed() {
        Ok(s) => s,
        Err(e) => {
            show_error_box(&format!("Could not load the settings:\n{e}"));
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
        .find(|p| p.name == preset_name)
        .or_else(|| {
            settings
                .conversion_presets
                .iter()
                .find(|p| p.name.eq_ignore_ascii_case(preset_name))
        }) {
        Some(p) => p.clone(),
        None => {
            show_error_box(&format!(
                "The preset \"{preset_name}\" does not exist (it may have been renamed or deleted).\n\nOpen File Converter's settings to see the available presets."
            ));
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

    window.set_dark_mode(settings.dark_mode.unwrap_or_else(is_windows_dark_mode));
    // Persist the theme choice immediately (this window has no Save button).
    // Only the theme is written, so an open settings window's unsaved edits
    // and the file's other values are left alone.
    window.on_toggle_theme(|dark| persist_theme_preference(Some(dark)));
    window.set_preset_name(preset.name.as_str().into());
    window.set_overall_progress(0.0);
    window.set_overall_status_text("Starting conversion...".into());

    // One model for the window's whole life. Rows are updated in place and
    // only when they changed: rebuilding the whole model every 100 ms (as
    // before) re-created every row widget, reset the scroll position and cost
    // O(files) allocations per tick.
    let rows: Vec<JobProgressData> = scheduler.jobs.iter().map(job_row).collect();
    let model = Rc::new(slint::VecModel::from(rows));
    window.set_jobs(model.clone().into());

    let auto_close = settings.exit_application_when_conversions_finished;
    let exit_delay = settings
        .duration_between_end_of_conversions_and_application_exit
        .clamp(0.0, 10.0);
    let batch_start = Instant::now();
    let finished_at = Rc::new(std::cell::Cell::new(None::<Instant>));
    let keep_open = Rc::new(std::cell::Cell::new(false));
    // Rows that reached a final state and were rendered once need no updates.
    let settled = Rc::new(std::cell::RefCell::new(vec![false; scheduler.jobs.len()]));

    let preset_name_clone = preset.name.clone();

    // Slint Timer for Live UI Progress Updates (100ms interval)
    let timer = slint::Timer::default();
    let window_weak = window.as_weak();
    let scheduler_timer = scheduler.clone();
    let finished_timer = finished_at.clone();
    let keep_open_timer = keep_open.clone();
    let model_timer = model.clone();
    // Set once the batch is over and no countdown runs: the ticks then have
    // nothing to do, so they skip the per-job scan entirely.
    let idle_timer = std::cell::Cell::new(false);

    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(100),
        move || {
            if idle_timer.get() {
                return;
            }
            let Some(w) = window_weak.upgrade() else {
                return;
            };

            let total_count = scheduler_timer.jobs.len();
            let mut total_prog = 0.0f32;
            let mut done_count = 0;
            let mut failed_count = 0;
            let mut canceled_count = 0;

            let mut settled = settled.borrow_mut();
            for (i, job) in scheduler_timer.jobs.iter().enumerate() {
                let status = job.status.lock().clone();
                let is_final = matches!(
                    status,
                    JobStatus::Done | JobStatus::Failed(_) | JobStatus::Canceled
                );
                match status {
                    JobStatus::Done => {
                        done_count += 1;
                        total_prog += 1.0;
                    }
                    JobStatus::Failed(_) => {
                        failed_count += 1;
                        total_prog += 1.0;
                    }
                    JobStatus::Canceled => {
                        canceled_count += 1;
                        total_prog += 1.0;
                    }
                    _ => total_prog += job.get_progress().clamp(0.0, 1.0),
                }

                if settled[i] {
                    continue;
                }
                let row = job_row(job);
                if model_timer.row_data(i).as_ref() != Some(&row) {
                    model_timer.set_row_data(i, row);
                }
                // A finished job is rendered one last time, then skipped; its
                // timing is final once the worker released it.
                if is_final && job.timing.lock().finished.is_some() {
                    settled[i] = true;
                }
            }
            drop(settled);

            let completed_count = done_count + failed_count + canceled_count;
            let overall = if total_count > 0 {
                total_prog / total_count as f32
            } else {
                1.0
            };
            let all_finished = completed_count >= total_count;

            let status_summary = if all_finished {
                let mut text = if failed_count == 0 && canceled_count == 0 {
                    format!("All {total_count} file(s) converted successfully")
                } else {
                    let mut parts = vec![format!("{done_count} converted")];
                    if failed_count > 0 {
                        parts.push(format!("{failed_count} failed"));
                    }
                    if canceled_count > 0 {
                        parts.push(format!("{canceled_count} canceled"));
                    }
                    parts.join(", ")
                };
                text.push_str(&format!(" · {}", format_duration(batch_start.elapsed())));
                text
            } else {
                let mut text = format!("{completed_count} of {total_count} file(s) finished");
                if failed_count > 0 {
                    text.push_str(&format!(" · {failed_count} failed"));
                }
                let elapsed = batch_start.elapsed();
                if overall > 0.02 && elapsed > Duration::from_secs(2) {
                    let left = elapsed.as_secs_f32() * (1.0 - overall) / overall;
                    text.push_str(&format!(
                        " · about {} left",
                        format_duration(Duration::from_secs_f32(left))
                    ));
                }
                text
            };

            w.set_overall_status_text(status_summary.into());
            w.set_overall_progress(overall);

            if !all_finished {
                return;
            }

            let all_succeeded = done_count == total_count;
            if finished_timer.get().is_none() {
                finished_timer.set(Some(Instant::now()));
                w.set_is_finished(true);
                w.set_has_failures(!all_succeeded);
                play_completion_sound();
                // Set once: rewriting it every tick (as before) wiped out
                // messages such as "Copied 3 files to the clipboard".
                if !all_succeeded {
                    w.set_info_message(
                        "Some files were not converted (see the messages above). \"Retry failed\" tries them again."
                            .into(),
                    );
                }

                let records = scheduler_timer
                    .jobs
                    .iter()
                    .map(|job| HistoryRecord {
                        timestamp: history_timestamp(),
                        preset_name: preset_name_clone.clone(),
                        input_path: job.input_path.clone(),
                        output_path: job.output_file_paths.join("; "),
                        status: match &*job.status.lock() {
                            JobStatus::Done => "Done".to_string(),
                            JobStatus::Failed(e) => format!("Failed ({})", e),
                            JobStatus::Canceled => "Canceled".to_string(),
                            _ => "Finished".to_string(),
                        },
                    })
                    .collect();
                add_history_records(records);
            }

            // Like the C# original, only close by itself when *everything*
            // succeeded; otherwise the errors would vanish with the window.
            if !auto_close || keep_open_timer.get() || !all_succeeded {
                w.set_auto_close_pending(false);
                // Nothing left to update: every row has settled.
                idle_timer.set(true);
                return;
            }

            let since = finished_timer
                .get()
                .map(|t| t.elapsed().as_secs_f32())
                .unwrap_or(0.0);
            let remaining = exit_delay - since;
            if remaining <= 0.0 {
                // Must not panic: this runs inside the Slint timer callback,
                // where a panic would abort the process.
                let _ = slint::quit_event_loop();
            } else {
                w.set_auto_close_pending(true);
                let secs = remaining.ceil() as u32;
                w.set_info_message(
                    format!(
                        "Closing in {secs} second{}…",
                        if secs == 1 { "" } else { "s" }
                    )
                    .into(),
                );
            }
        },
    );

    // Callbacks
    let scheduler_cancel = scheduler.clone();
    window.on_cancel_job(move |job_id| {
        if let Some(job) = scheduler_cancel
            .jobs
            .iter()
            .find(|job| job.id == job_id as usize)
        {
            job.cancel();
        }
    });

    let scheduler_cancel_all = scheduler.clone();
    let window_weak = window.as_weak();
    window.on_cancel_all(move || {
        scheduler_cancel_all.cancel_all();
        if let Some(w) = window_weak.upgrade() {
            w.set_info_message(
                "Canceling - files being converted stop at the next safe point…".into(),
            );
        }
    });

    let window_weak = window.as_weak();
    let keep_open_cb = keep_open.clone();
    window.on_keep_open(move || {
        keep_open_cb.set(true);
        if let Some(w) = window_weak.upgrade() {
            w.set_auto_close_pending(false);
            w.set_info_message("The window will stay open.".into());
        }
    });

    let scheduler_folder = scheduler.clone();
    window.on_open_output_folder(move || {
        // Open the folder of the first *successful* output, falling back to the
        // first job's folder. The file itself is pre-selected in Explorer.
        let first_existing = scheduler_folder
            .jobs
            .iter()
            .filter(|job| matches!(*job.status.lock(), JobStatus::Done))
            .flat_map(|job| job.output_file_paths.iter())
            .find(|p| Path::new(p).is_file())
            .cloned();

        if let Some(path) = first_existing {
            reveal_in_explorer(&path);
        } else if let Some(path) = scheduler_folder
            .jobs
            .first()
            .and_then(|j| j.output_file_paths.first().cloned())
        {
            reveal_in_explorer(&path);
        }
    });

    // "Show" on a converted row: that file, selected in Explorer.
    let scheduler_reveal = scheduler.clone();
    let window_weak = window.as_weak();
    window.on_reveal_output(move |job_id| {
        let path = scheduler_reveal
            .jobs
            .iter()
            .find(|job| job.id == job_id as usize)
            .and_then(|job| job.output_file_paths.first().cloned());
        let shown = path.as_deref().is_some_and(reveal_in_explorer);
        if !shown {
            if let Some(w) = window_weak.upgrade() {
                w.set_info_message("That file is no longer there.".into());
            }
        }
    });

    // "Retry failed": the failed and canceled files go to a fresh batch with
    // the same preset, and this window closes.
    let scheduler_retry = scheduler.clone();
    let retry_preset = preset.name.clone();
    window.on_retry_failed(move || {
        let files: Vec<String> = scheduler_retry
            .jobs
            .iter()
            .filter(|job| {
                matches!(
                    *job.status.lock(),
                    JobStatus::Failed(_) | JobStatus::Canceled
                )
            })
            .map(|job| job.input_path.clone())
            .filter(|p| Path::new(p).exists())
            .collect();
        if !files.is_empty() {
            spawn_conversion_process(&retry_preset, &files);
        }
        let _ = slint::quit_event_loop();
    });

    let scheduler_copy = scheduler.clone();
    let window_weak = window.as_weak();
    window.on_copy_output_paths(move || {
        // Only copy paths that actually exist.
        let all_outs: Vec<String> = scheduler_copy
            .jobs
            .iter()
            .flat_map(|job| job.output_file_paths.iter())
            .filter(|p| Path::new(p).is_file())
            .cloned()
            .collect();
        let message = if all_outs.is_empty() {
            "No converted files to copy yet.".to_string()
        } else {
            match file_converter_core::scheduler::copy_files_to_clipboard(&all_outs) {
                Ok(()) => format!("Copied {} file(s) to the clipboard.", all_outs.len()),
                Err(e) => format!("Could not copy to the clipboard: {e}"),
            }
        };
        if let Some(w) = window_weak.upgrade() {
            w.set_info_message(message.into());
        }
    });

    // Closing mid-conversion would terminate the detached worker thread and
    // truncate outputs, so the window is kept open until the batch settles.
    // This covers both the Close button and the title-bar X (which used to
    // close the window and kill running conversions).
    let still_running = {
        let scheduler = scheduler.clone();
        move || scheduler.jobs.iter().any(|job| job.is_active())
    };

    let window_close_cb = window.as_weak();
    let running_for_button = still_running.clone();
    window.on_close_window(move || {
        if let Some(w) = window_close_cb.upgrade() {
            if running_for_button() {
                w.set_info_message(
                    "Conversions are still running - press \"Cancel all\" to stop them, or wait for them to finish."
                        .into(),
                );
                return;
            }
            let _ = slint::quit_event_loop();
        }
    });

    let window_x_cb = window.as_weak();
    window.window().on_close_requested(move || {
        if still_running() {
            if let Some(w) = window_x_cb.upgrade() {
                w.set_info_message(
                    "Conversions are still running - press \"Cancel all\" to stop them, or wait for them to finish."
                        .into(),
                );
            }
            slint::CloseRequestResponse::KeepWindowShown
        } else {
            let _ = slint::quit_event_loop();
            slint::CloseRequestResponse::HideWindow
        }
    });

    let _ = window.run();
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
