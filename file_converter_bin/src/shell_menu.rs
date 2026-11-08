//! The Explorer right-click menu: whether it is set up correctly, and adding,
//! repairing or removing it.
//!
//! The menu itself lives in `file_converter_shell.dll`; registering that DLL
//! (`regsvr32`) also removes the leftovers of older versions. This module
//! reads the registry to tell the user, in plain words, what state the menu is
//! in, so "why don't I see my presets when I right-click?" has an answer and a
//! one-click fix in Preferences.

use std::path::{Path, PathBuf};

/// The shell extension's COM class id (same as the C# original).
pub const CLSID: &str = "{AF9B72B5-F4E4-44B0-A3D9-B55B748EFE90}";

/// Classes older versions put a flat "File Converter" verb under.
const VERB_ASSOCIATIONS: [&str; 6] = [
    "*",
    "AllFilesystemObjects",
    "Directory",
    "Directory\\Background",
    "Drive",
    "Folder",
];

/// What the registry says about the right-click menu.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Registration {
    /// The DLL Explorer loads for the menu, when the class is registered.
    pub dll: Option<PathBuf>,
    /// The menu handler is attached to files (`*\shellex\ContextMenuHandlers`).
    pub handler_on_files: bool,
    /// A flat "File Converter" verb from an older version is still there. It
    /// opened the settings window instead of listing presets.
    pub leftover_verb: bool,
}

/// The state of the menu, as shown in Preferences.
#[derive(Debug, Clone, PartialEq)]
pub enum MenuHealth {
    /// Working, using the DLL next to this program.
    Ready,
    /// Working, but with another copy of File Converter (another folder).
    OtherCopy(PathBuf),
    /// Never added (or removed).
    NotAdded,
    /// Registered, but the DLL it points to is gone (moved or uninstalled).
    MissingDll(PathBuf),
    /// An old entry from an earlier version is shadowing the preset menu.
    Leftovers,
}

impl MenuHealth {
    /// True when right-clicking files shows the preset menu.
    pub fn is_ok(&self) -> bool {
        matches!(self, MenuHealth::Ready | MenuHealth::OtherCopy(_))
    }

    /// True when something is registered (so there is something to remove).
    pub fn is_added(&self) -> bool {
        !matches!(self, MenuHealth::NotAdded)
    }

    /// Needs fixing: shown as a warning, and announced when the window opens.
    pub fn is_broken(&self) -> bool {
        matches!(self, MenuHealth::MissingDll(_) | MenuHealth::Leftovers)
    }

    pub fn message(&self) -> String {
        const WHERE: &str = "On Windows 11 it is under \"Show more options\" (or hold Shift while you right-click).";
        match self {
            MenuHealth::Ready => format!(
                "Ready. Right-click files in Explorer and choose File Converter to convert them with a preset. {WHERE}"
            ),
            MenuHealth::OtherCopy(dll) => format!(
                "Working, but it uses the copy of File Converter in {}. Choose \"Add again\" to use this copy instead.",
                dll.parent().unwrap_or(dll).display()
            ),
            MenuHealth::NotAdded => {
                "Not added yet. Choose \"Add to the menu\" to convert files straight from Explorer's right-click menu."
                    .to_string()
            }
            MenuHealth::MissingDll(dll) => format!(
                "Broken: the menu points to {}, which no longer exists. Choose \"Repair the menu\".",
                dll.display()
            ),
            MenuHealth::Leftovers => "An old \"File Converter\" entry from an earlier version opens this window instead of listing your presets. Choose \"Repair the menu\" to remove it."
                .to_string(),
        }
    }

    /// Label of the main button in Preferences.
    pub fn action_label(&self) -> &'static str {
        match self {
            MenuHealth::NotAdded => "Add to the menu",
            MenuHealth::Ready | MenuHealth::OtherCopy(_) => "Add again",
            MenuHealth::MissingDll(_) | MenuHealth::Leftovers => "Repair the menu",
        }
    }
}

fn same_path(a: &Path, b: &Path) -> bool {
    a.to_string_lossy()
        .eq_ignore_ascii_case(&b.to_string_lossy())
}

/// Decides the menu state from what the registry says.
pub fn evaluate(
    reg: &Registration,
    our_dll: &Path,
    file_exists: impl Fn(&Path) -> bool,
) -> MenuHealth {
    if reg.leftover_verb {
        return MenuHealth::Leftovers;
    }
    let Some(dll) = reg.dll.as_ref().filter(|_| reg.handler_on_files) else {
        return MenuHealth::NotAdded;
    };
    if !file_exists(dll) {
        return MenuHealth::MissingDll(dll.clone());
    }
    if !same_path(dll, our_dll) && file_exists(our_dll) {
        return MenuHealth::OtherCopy(dll.clone());
    }
    MenuHealth::Ready
}

/// The shell DLL that ships next to this program.
pub fn our_dll() -> PathBuf {
    let mut dir = std::env::current_exe().unwrap_or_default();
    dir.pop();
    dir.join("file_converter_shell.dll")
}

#[cfg(target_os = "windows")]
pub fn read_registration() -> Registration {
    use winreg::RegKey;
    use winreg::enums::HKEY_CLASSES_ROOT;

    let hkcr = RegKey::predef(HKEY_CLASSES_ROOT);
    let dll = hkcr
        .open_subkey(format!("CLSID\\{CLSID}\\InprocServer32"))
        .and_then(|k| k.get_value::<String, _>(""))
        .ok()
        .map(|s| s.trim().trim_matches('"').to_string())
        .filter(|s| !s.is_empty())
        .map(PathBuf::from);
    let handler_on_files = hkcr
        .open_subkey("*\\shellex\\ContextMenuHandlers\\FileConverter")
        .is_ok();
    let leftover_verb = VERB_ASSOCIATIONS.iter().any(|assoc| {
        hkcr.open_subkey(format!("{assoc}\\shell\\FileConverter"))
            .is_ok()
    });
    Registration {
        dll,
        handler_on_files,
        leftover_verb,
    }
}

#[cfg(not(target_os = "windows"))]
pub fn read_registration() -> Registration {
    Registration::default()
}

/// The current state of the menu on this PC.
pub fn current() -> MenuHealth {
    evaluate(&read_registration(), &our_dll(), Path::exists)
}

/// Runs `regsvr32` elevated on the shell DLL and waits for it to finish.
///
/// Registering writes HKLM, so Windows asks for administrator permission.
/// It used to return as soon as the prompt appeared, so the app could not
/// tell whether anything changed; it now reports the real outcome.
#[cfg(target_os = "windows")]
pub fn run_regsvr32(unregister: bool) -> Result<(), String> {
    use windows::Win32::Foundation::{CloseHandle, ERROR_CANCELLED, WAIT_OBJECT_0};
    use windows::Win32::System::Threading::{GetExitCodeProcess, INFINITE, WaitForSingleObject};
    use windows::Win32::UI::Shell::{SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW};
    use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;
    use windows::core::PCWSTR;

    let dll_path = our_dll();
    if !dll_path.exists() {
        return Err(format!(
            "The menu add-on (file_converter_shell.dll) is missing from {}. Reinstall File Converter.",
            dll_path.parent().unwrap_or(&dll_path).display()
        ));
    }

    let wide = |s: &str| -> Vec<u16> { s.encode_utf16().chain(std::iter::once(0)).collect() };
    let switches = if unregister { "/u /s" } else { "/s" };
    let params = wide(&format!("{switches} \"{}\"", dll_path.display()));
    let verb = wide("runas");
    let file = wide("regsvr32.exe");

    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS,
        hwnd: crate::owner_hwnd(),
        lpVerb: PCWSTR(verb.as_ptr()),
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: PCWSTR(params.as_ptr()),
        nShow: SW_HIDE.0,
        ..Default::default()
    };

    // SAFETY: `info` and every string it points to live until the call
    // returns; the process handle it yields is closed below.
    if let Err(e) = unsafe { ShellExecuteExW(&mut info) } {
        return Err(if e.code() == ERROR_CANCELLED.to_hresult() {
            "Nothing was changed: the Windows permission prompt was declined.".to_string()
        } else {
            format!("Couldn't start regsvr32: {}", e.message())
        });
    }
    if info.hProcess.is_invalid() {
        return Ok(());
    }

    // SAFETY: `hProcess` is a live process handle owned by us.
    let code = unsafe {
        let _close = scopeguard::guard(info.hProcess, |h| {
            let _ = CloseHandle(h);
        });
        if WaitForSingleObject(info.hProcess, INFINITE) != WAIT_OBJECT_0 {
            return Err("Lost track of regsvr32 while it was running.".to_string());
        }
        let mut code = 0u32;
        let _ = GetExitCodeProcess(info.hProcess, &mut code);
        code
    };

    match code {
        0 => Ok(()),
        3 | 4 => Err(
            "Windows couldn't load the menu add-on. Reinstall File Converter and try again."
                .to_string(),
        ),
        _ => Err(format!(
            "Windows reported an error ({code}) while updating the menu. Try again, or reinstall File Converter."
        )),
    }
}

#[cfg(not(target_os = "windows"))]
pub fn run_regsvr32(_unregister: bool) -> Result<(), String> {
    Err("The Explorer menu is only available on Windows.".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reg(dll: Option<&str>, handler: bool, leftover: bool) -> Registration {
        Registration {
            dll: dll.map(PathBuf::from),
            handler_on_files: handler,
            leftover_verb: leftover,
        }
    }

    const OURS: &str = "C:\\Program Files\\FileConverter\\file_converter_shell.dll";

    #[test]
    fn ready_when_our_dll_is_registered() {
        let health = evaluate(
            &reg(Some(&OURS.to_uppercase()), true, false),
            Path::new(OURS),
            |_| true,
        );
        assert_eq!(health, MenuHealth::Ready);
        assert!(health.is_ok() && health.is_added() && !health.is_broken());
    }

    #[test]
    fn leftover_verb_wins_even_when_the_menu_is_registered() {
        let health = evaluate(&reg(Some(OURS), true, true), Path::new(OURS), |_| true);
        assert_eq!(health, MenuHealth::Leftovers);
        assert!(health.is_broken());
        assert_eq!(health.action_label(), "Repair the menu");
    }

    #[test]
    fn not_added_without_class_or_handler() {
        assert_eq!(
            evaluate(&reg(None, true, false), Path::new(OURS), |_| true),
            MenuHealth::NotAdded
        );
        assert_eq!(
            evaluate(&reg(Some(OURS), false, false), Path::new(OURS), |_| true),
            MenuHealth::NotAdded
        );
    }

    #[test]
    fn missing_dll_is_broken() {
        let health = evaluate(
            &reg(Some("D:\\Old\\file_converter_shell.dll"), true, false),
            Path::new(OURS),
            |p| p == Path::new(OURS),
        );
        assert!(matches!(health, MenuHealth::MissingDll(_)));
        assert!(health.message().contains("D:\\Old"));
    }

    #[test]
    fn another_copy_works_but_is_reported() {
        let health = evaluate(
            &reg(Some("D:\\Portable\\file_converter_shell.dll"), true, false),
            Path::new(OURS),
            |_| true,
        );
        assert!(matches!(health, MenuHealth::OtherCopy(_)));
        assert!(health.is_ok());
        assert!(health.message().contains("D:\\Portable"));
    }
}
