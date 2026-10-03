#![allow(
    non_snake_case,
    non_camel_case_types,
    unsafe_op_in_unsafe_fn,
    clippy::missing_safety_doc,
    clippy::collapsible_if,
    clippy::upper_case_acronyms,
    clippy::let_and_return,
    clippy::useless_conversion
)]

use std::ffi::{OsString, c_void};
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::{LazyLock, RwLock};
use std::time::SystemTime;

use file_converter_core::settings::{ConversionPreset, Settings};
use file_converter_core::types::{OutputType, is_preset_applicable_to_file};

#[allow(unused_imports)]
mod windows_core {
    pub use windows::core::*;
}
use windows::Win32::Foundation::{
    BOOL, CLASS_E_CLASSNOTAVAILABLE, CLASS_E_NOAGGREGATION, E_FAIL, HMODULE, LPARAM, S_FALSE, S_OK,
};
use windows::Win32::Graphics::Gdi::{CreateBitmap, DeleteObject, HBITMAP};
use windows::Win32::System::Com::{
    CoTaskMemAlloc, CoTaskMemFree, DVASPECT_CONTENT, FORMATETC, IBindCtx, IClassFactory,
    IClassFactory_Impl, IDataObject, STGMEDIUM, TYMED_HGLOBAL,
};
use windows::Win32::System::Diagnostics::Debug::OutputDebugStringW;
use windows::core::{GUID, HRESULT, Interface, PCWSTR, PSTR, PWSTR, Result, implement};

#[link(name = "ole32")]
unsafe extern "system" {
    fn ReleaseStgMedium(pmedium: *mut STGMEDIUM);
}

/// Debug logging helper for the shell extension (visible in debuggers / DebugView).
macro_rules! dbg_log {
    ($($arg:tt)*) => {{
        #[cfg(target_os = "windows")]
        {
            let msg = format!($($arg)*);
            let wide: Vec<u16> = msg.encode_utf16().chain(std::iter::once(0)).collect();
            unsafe { OutputDebugStringW(PCWSTR(wide.as_ptr())); }
        }
    }};
}

use windows::Win32::System::LibraryLoader::{
    DisableThreadLibraryCalls, GetModuleFileNameW, GetModuleHandleW,
};
use windows::Win32::System::Memory::{GlobalLock, GlobalUnlock};
use windows::Win32::System::Registry::HKEY;
use windows::Win32::UI::Controls::HPROPSHEETPAGE;
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    CMINVOKECOMMANDINFO, DragQueryFileW, ECF_DEFAULT, ECS_ENABLED, HDROP, IContextMenu,
    IContextMenu_Impl, IEnumExplorerCommand, IExplorerCommand, IExplorerCommand_Impl,
    IShellExtInit, IShellExtInit_Impl, IShellItemArray, IShellPropSheetExt,
    IShellPropSheetExt_Impl, SHCNE_ASSOCCHANGED, SHCNF_IDLIST, SHChangeNotify, SIGDN_FILESYSPATH,
};

type LPFNADDPROPSHEETPAGE = Option<unsafe extern "system" fn(HPROPSHEETPAGE, LPARAM) -> BOOL>;
use windows::Win32::UI::WindowsAndMessaging::{
    CreatePopupMenu, DestroyMenu, HMENU, InsertMenuItemW, MENUITEMINFOW, MFT_SEPARATOR, MFT_STRING,
    MIIM_BITMAP, MIIM_FTYPE, MIIM_ID, MIIM_STRING, MIIM_SUBMENU,
};

const CLSID_FILE_CONVERTER: GUID = GUID::from_u128(0xAF9B72B5_F4E4_44B0_A3D9_B55B748EFE90);

// Atomic DLL instance handle & active COM object counters
static G_DLL_INSTANCE: AtomicUsize = AtomicUsize::new(0);
static G_LOCK_COUNT: AtomicU32 = AtomicU32::new(0);
static G_OBJECT_COUNT: AtomicU32 = AtomicU32::new(0);

#[unsafe(no_mangle)]
pub unsafe extern "system" fn DllMain(
    hinst_dll: HMODULE,
    fdw_reason: u32,
    _lpv_reserved: *mut c_void,
) -> i32 {
    if fdw_reason == 1 {
        // DLL_PROCESS_ATTACH
        G_DLL_INSTANCE.store(hinst_dll.0 as usize, Ordering::Relaxed);

        // No allocation, no formatting and no locking here: `DllMain` runs under
        // the loader lock, where a panic cannot unwind across the `extern
        // "system"` boundary and would abort the host process (explorer.exe).
        // Diagnostic output is emitted lazily, outside of DllMain.
        //
        // Also skip per-thread notifications: this is an in-process COM server
        // and Explorer can attach thousands of threads over a session.
        let _ = DisableThreadLibraryCalls(hinst_dll);
    }
    1
}

/// The single canonical copy of the default presets, embedded at build time.
///
/// It lives at the repository root (shared with `file_converter_bin`, the
/// installer and the release packaging). Do **not** add a crate-local copy: the
/// duplicates that used to exist here silently shipped stale presets.
const DEFAULT_SETTINGS_XML: &str = include_str!("../../Settings.default.xml");

const fn rgb(r: u8, g: u8, b: u8) -> u32 {
    ((r as u32) << 16) | ((g as u32) << 8) | (b as u32)
}

fn create_category_icon(output_type: OutputType) -> HBITMAP {
    // 32bpp GDI Bitmap memory layout: Byte 0 = B, Byte 1 = G, Byte 2 = R, Byte 3 = 0
    let color: u32 = match output_type {
        OutputType::Aac
        | OutputType::Flac
        | OutputType::Mp3
        | OutputType::Ogg
        | OutputType::Wav => rgb(255, 140, 0), // Vibrant Amber / Orange (Audio)
        OutputType::Avi
        | OutputType::Mkv
        | OutputType::Mp4
        | OutputType::Ogv
        | OutputType::Webm => rgb(32, 96, 224), // Vivid Blue (Video)
        OutputType::Avif
        | OutputType::Ico
        | OutputType::Jpg
        | OutputType::Jxl
        | OutputType::Png
        | OutputType::Webp
        | OutputType::Gif => rgb(48, 176, 32), // Emerald Green (Image)
        OutputType::Pdf => rgb(216, 32, 32), // Crimson Red (PDF / Document)
        _ => rgb(112, 112, 112),             // Slate Gray
    };

    let mut pixels = [color; 16 * 16];
    let border_color = rgb(48, 48, 48);
    for y in 0..16 {
        for x in 0..16 {
            if x == 0 || x == 15 || y == 0 || y == 15 {
                pixels[y * 16 + x] = border_color;
            }
        }
    }
    // SAFETY: pixels is a valid 16x16 32bpp buffer that remains alive during CreateBitmap.
    unsafe { CreateBitmap(16, 16, 1, 32, Some(pixels.as_ptr() as *const c_void)) }
}

fn get_selected_files_from_data_object(data_obj: &IDataObject) -> Vec<String> {
    let mut files = Vec::new();
    let fmt = FORMATETC {
        cfFormat: 15, // CF_HDROP
        ptd: std::ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32,
    };
    // SAFETY: Interrogating IDataObject for CF_HDROP and reading wide file paths with RAII cleanup.
    unsafe {
        if let Ok(mut medium) = data_obj.GetData(&fmt) {
            let h_drop = medium.u.hGlobal;

            // Guarantee the STGMEDIUM is released on every exit path.
            let _release = scopeguard::guard((), |_| ReleaseStgMedium(&mut medium));

            if !h_drop.0.is_null() {
                let ptr = GlobalLock(h_drop);
                if !ptr.is_null() {
                    let _unlock = scopeguard::guard((), |_| {
                        let _ = GlobalUnlock(h_drop);
                    });

                    let file_count = DragQueryFileW(HDROP(ptr), 0xFFFFFFFF, None);
                    for i in 0..file_count {
                        let size = DragQueryFileW(HDROP(ptr), i, None);
                        if size > 0 {
                            let mut buf = vec![0u16; size as usize + 1];
                            let written = DragQueryFileW(HDROP(ptr), i, Some(&mut buf));
                            // The fill must actually happen: otherwise `buf` stays
                            // zeroed and yields an empty path, which would make
                            // every preset look compatible with the selection.
                            if written == 0 {
                                continue;
                            }
                            let len = (written as usize).min(buf.len() - 1);
                            if let Some(null_pos) = buf[..len].iter().position(|&x| x == 0) {
                                let os_str = OsString::from_wide(&buf[..null_pos]);
                                if let Ok(path_str) = os_str.into_string()
                                    // Folders and other non-file items have no
                                    // extension, so they would match every preset.
                                    && Path::new(&path_str).is_file()
                                {
                                    files.push(path_str);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    files
}

#[implement(IShellExtInit, IContextMenu, IShellPropSheetExt, IExplorerCommand)]
struct FileConverterShellExt {
    selected_files: RwLock<Vec<String>>,
    active_presets: RwLock<Vec<String>>,
    configure_cmd_offset: RwLock<Option<usize>>,
}

impl FileConverterShellExt {
    fn new() -> Self {
        G_OBJECT_COUNT.fetch_add(1, Ordering::Relaxed);
        Self {
            selected_files: RwLock::new(Vec::new()),
            active_presets: RwLock::new(Vec::new()),
            configure_cmd_offset: RwLock::new(None),
        }
    }
}

impl Drop for FileConverterShellExt {
    fn drop(&mut self) {
        G_OBJECT_COUNT.fetch_sub(1, Ordering::Relaxed);
    }
}

impl IShellExtInit_Impl for FileConverterShellExt_Impl {
    fn Initialize(
        &self,
        _pidlfolder: *const ITEMIDLIST,
        pdtobj: Option<&IDataObject>,
        _hkeyprogid: HKEY,
    ) -> Result<()> {
        if let Some(data_obj) = pdtobj {
            let files = get_selected_files_from_data_object(data_obj);
            if let Ok(mut lock) = self.selected_files.write() {
                *lock = files;
            }
            Ok(())
        } else {
            Err(E_FAIL.into())
        }
    }
}

// In-memory thread-safe cached settings loader to avoid disk I/O on UI thread
static CACHED_SETTINGS: LazyLock<RwLock<Option<(SystemTime, Settings)>>> =
    LazyLock::new(|| RwLock::new(None));

fn get_cached_settings() -> Settings {
    let local_app_data = std::env::var("LOCALAPPDATA").unwrap_or_default();
    let user_settings_path = Path::new(&local_app_data)
        .join("FileConverter")
        .join("Settings.user.xml");

    let mtime = user_settings_path
        .metadata()
        .and_then(|m| m.modified())
        .unwrap_or(SystemTime::UNIX_EPOCH);

    if let Ok(guard) = CACHED_SETTINGS.read() {
        if let Some((cached_mtime, ref settings)) = *guard {
            if cached_mtime == mtime {
                return settings.clone();
            }
        }
    }

    let loaded = if user_settings_path.exists() {
        Settings::load_from_file(&user_settings_path).unwrap_or_else(|_| create_default_settings())
    } else {
        create_default_settings()
    };

    if let Ok(mut guard) = CACHED_SETTINGS.write() {
        *guard = Some((mtime, loaded.clone()));
    }

    loaded
}

fn is_preset_compatible_with_file(preset: &ConversionPreset, file_path: &str) -> bool {
    // Single source of truth: the same check the scheduler uses before queueing
    // a job, so the context menu can never offer a preset that would be rejected.
    let declared: Vec<String> = preset.input_types.iter().map(|s| s.to_string()).collect();
    is_preset_applicable_to_file(preset.output_type, &declared, file_path)
}

/// Writes the selection to a uniquely named temp file and returns its path.
///
/// This DLL runs *inside* `explorer.exe`, so `std::process::id()` is the same for
/// every context-menu invocation. Keying the file on it alone made two concurrent
/// right-clicks overwrite each other's list, so the wrong files were converted
/// with the wrong preset. The name is now unique per invocation and the file is
/// created exclusively (never truncating a pre-existing file).
fn write_selection_to_temp_list(
    preset_name: &str,
    selected_files: &[String],
) -> Option<std::path::PathBuf> {
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let temp_dir = std::env::temp_dir();
    for _ in 0..8 {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
        let candidate = temp_dir.join(format!(
            "file-converter-input-list-{}-{}-{}.txt",
            std::process::id(),
            nanos,
            seq
        ));

        // `create_new` fails if the path already exists, so an attacker-planted
        // file (or a concurrent invocation) is never truncated or followed.
        let Ok(mut file) = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        else {
            continue;
        };

        let mut ok = true;
        for path in selected_files {
            if writeln!(file, "{}", path).is_err() {
                ok = false;
                break;
            }
        }
        if file.flush().is_err() {
            ok = false;
        }
        drop(file);

        if ok {
            return Some(candidate);
        }

        let _ = std::fs::remove_file(&candidate);
    }

    dbg_log!(
        "Failed to write temporary input list for preset '{}'",
        preset_name
    );
    None
}

impl IContextMenu_Impl for FileConverterShellExt_Impl {
    fn QueryContextMenu(
        &self,
        hmenu: HMENU,
        indexmenu: u32,
        idcmdfirst: u32,
        _idcmdlast: u32,
        uflags: u32,
    ) -> Result<()> {
        const CMF_DEFAULTONLY: u32 = 0x0001;

        if uflags & CMF_DEFAULTONLY != 0 {
            return Ok(());
        }

        let selected_files = match self.selected_files.read() {
            Ok(lock) => lock.clone(),
            Err(_) => return Ok(()),
        };

        if selected_files.is_empty() {
            return Ok(());
        }

        let mut settings = get_cached_settings();
        settings.merge(create_default_settings());

        let compatible_presets: Vec<_> = settings
            .conversion_presets
            .into_iter()
            .filter(|preset| {
                selected_files
                    .iter()
                    .all(|file| is_preset_compatible_with_file(preset, file))
            })
            .collect();

        if compatible_presets.is_empty() {
            return Ok(());
        }

        if let Ok(mut lock) = self.active_presets.write() {
            *lock = compatible_presets.iter().map(|p| p.name.clone()).collect();
        }
        if let Ok(mut lock) = self.configure_cmd_offset.write() {
            *lock = None;
        }

        let presets_count = compatible_presets.len();
        let needed_count = if presets_count <= 5 {
            presets_count as u32
        } else {
            (presets_count + 2) as u32
        };

        // The shell reserves the command-id range `[idcmdfirst, _idcmdlast]` for
        // this handler. If it is not large enough we must add nothing at all:
        // emitting ids outside the range collides with the next handler and
        // dispatches `lpVerb` offsets that were never displayed.
        //
        // The previous guard `if _idcmdlast >= idcmdfirst && (...)` short-circuited
        // to *false* when no range was reserved, letting the fall-through path run.
        let fits = idcmdfirst
            .checked_add(needed_count)
            .is_some_and(|end| end <= _idcmdlast);
        if !fits {
            return Ok(());
        }

        let cmd_id = idcmdfirst;
        let configure_cmd_id = cmd_id + presets_count as u32;

        let mut parent_text_wide: Vec<u16> = "File Converter\0".encode_utf16().collect();

        unsafe {
            if presets_count <= 5 {
                // Count only the items Explorer was actually told about.
                let mut inserted_count: u32 = 0;
                for (i, preset) in compatible_presets.iter().enumerate() {
                    let mut name_wide: Vec<u16> = preset.name.encode_utf16().collect();
                    name_wide.push(0);

                    let icon_bmp = create_category_icon(preset.output_type);

                    // The menu takes ownership of the bitmap on success; on failure
                    // the guard deletes it to avoid a GDI handle leak.
                    let bitmap_guard = scopeguard::guard(icon_bmp, |b| {
                        let _ = DeleteObject(b);
                    });

                    let mii = MENUITEMINFOW {
                        cbSize: std::mem::size_of::<MENUITEMINFOW>() as u32,
                        fMask: MIIM_STRING | MIIM_ID | MIIM_FTYPE | MIIM_BITMAP,
                        fType: MFT_STRING,
                        wID: cmd_id + i as u32,
                        dwTypeData: windows::core::PWSTR(name_wide.as_mut_ptr()),
                        cch: (name_wide.len() - 1) as u32,
                        hbmpItem: icon_bmp,
                        ..Default::default()
                    };

                    let inserted = InsertMenuItemW(hmenu, indexmenu + i as u32, true, &mii);
                    if inserted.is_ok() {
                        scopeguard::ScopeGuard::into_inner(bitmap_guard);
                        inserted_count += 1;
                    }
                }

                Err(windows::core::Error::from_hresult(HRESULT(
                    inserted_count as i32,
                )))
            } else {
                let h_sub_menu = CreatePopupMenu()?;

                // `DestroyMenu` on the error path: without this the USER handle
                // leaked on every failed insertion (a per-right-click leak inside
                // the long-lived explorer.exe process).
                let sub_menu_guard = scopeguard::guard(h_sub_menu, |h| {
                    let _ = DestroyMenu(h);
                });

                let mut inserted_count: u32 = 0;
                for (i, preset) in compatible_presets.iter().enumerate() {
                    let mut name_wide: Vec<u16> = preset.name.encode_utf16().collect();
                    name_wide.push(0);

                    let icon_bmp = create_category_icon(preset.output_type);

                    let bitmap_guard = scopeguard::guard(icon_bmp, |b| {
                        let _ = DeleteObject(b);
                    });

                    let mii = MENUITEMINFOW {
                        cbSize: std::mem::size_of::<MENUITEMINFOW>() as u32,
                        fMask: MIIM_STRING | MIIM_ID | MIIM_FTYPE | MIIM_BITMAP,
                        fType: MFT_STRING,
                        wID: cmd_id + i as u32,
                        dwTypeData: windows::core::PWSTR(name_wide.as_mut_ptr()),
                        cch: (name_wide.len() - 1) as u32,
                        hbmpItem: icon_bmp,
                        ..Default::default()
                    };

                    let inserted = InsertMenuItemW(h_sub_menu, i as u32, true, &mii);
                    if inserted.is_ok() {
                        scopeguard::ScopeGuard::into_inner(bitmap_guard);
                        inserted_count += 1;
                    }
                }

                let sep_mii = MENUITEMINFOW {
                    cbSize: std::mem::size_of::<MENUITEMINFOW>() as u32,
                    fMask: MIIM_FTYPE,
                    fType: MFT_SEPARATOR,
                    ..Default::default()
                };
                if InsertMenuItemW(h_sub_menu, inserted_count, true, &sep_mii).is_ok() {
                    inserted_count += 1;
                }

                let mut config_text_wide: Vec<u16> = "Configure...\0".encode_utf16().collect();
                let config_mii = MENUITEMINFOW {
                    cbSize: std::mem::size_of::<MENUITEMINFOW>() as u32,
                    fMask: MIIM_STRING | MIIM_ID | MIIM_FTYPE,
                    fType: MFT_STRING,
                    wID: configure_cmd_id,
                    dwTypeData: windows::core::PWSTR(config_text_wide.as_mut_ptr()),
                    cch: (config_text_wide.len() - 1) as u32,
                    ..Default::default()
                };
                if InsertMenuItemW(h_sub_menu, inserted_count, true, &config_mii).is_ok() {
                    inserted_count += 1;
                }

                if let Ok(mut lock) = self.configure_cmd_offset.write() {
                    *lock = Some(presets_count);
                }

                let parent_cmd_id = configure_cmd_id + 1;
                let parent_mii = MENUITEMINFOW {
                    cbSize: std::mem::size_of::<MENUITEMINFOW>() as u32,
                    fMask: MIIM_STRING | MIIM_SUBMENU | MIIM_ID | MIIM_FTYPE,
                    fType: MFT_STRING,
                    wID: parent_cmd_id,
                    hSubMenu: h_sub_menu,
                    dwTypeData: windows::core::PWSTR(parent_text_wide.as_mut_ptr()),
                    cch: (parent_text_wide.len() - 1) as u32,
                    ..Default::default()
                };

                if InsertMenuItemW(hmenu, indexmenu, true, &parent_mii).is_ok() {
                    // The menu now owns the popup, so drop the destruction guard.
                    let h_sub_menu = scopeguard::ScopeGuard::into_inner(sub_menu_guard);
                    let _ = h_sub_menu;
                    inserted_count += 1;
                }

                Err(windows::core::Error::from_hresult(HRESULT(
                    inserted_count as i32,
                )))
            }
        }
    }

    fn InvokeCommand(&self, pici: *const CMINVOKECOMMANDINFO) -> Result<()> {
        if pici.is_null() {
            return Err(E_FAIL.into());
        }

        let verb_val = unsafe { (*pici).lpVerb.0 as usize };
        if verb_val >> 16 != 0 {
            return Err(E_FAIL.into());
        }
        let verb_offset = verb_val & 0xFFFF;

        let selected_files = self
            .selected_files
            .read()
            .map(|l| l.clone())
            .unwrap_or_default();
        let mut presets = self
            .active_presets
            .read()
            .map(|l| l.clone())
            .unwrap_or_default();

        if presets.is_empty() {
            let mut settings = get_cached_settings();
            settings.merge(create_default_settings());

            presets = settings
                .conversion_presets
                .into_iter()
                .filter(|preset| {
                    // An empty selection would make `.all()` vacuously true and
                    // offer every preset, so bail out instead of guessing.
                    !selected_files.is_empty()
                        && selected_files
                            .iter()
                            .all(|file| is_preset_compatible_with_file(preset, file))
                })
                .map(|p| p.name)
                .collect();
        }

        let presets_count = presets.len();
        dbg_log!(
            "InvokeCommand verb_offset={} presets_count={}",
            verb_offset,
            presets_count
        );

        if verb_offset < presets_count {
            let preset_name = &presets[verb_offset];

            let bin_path = get_bin_path();
            if !bin_path.exists() {
                dbg_log!("Converter binary not found: {}", bin_path.display());
                return Err(E_FAIL.into());
            }

            let mut cmd = Command::new(&bin_path);
            cmd.arg("--conversion-preset").arg(preset_name);

            let mut total_len = preset_name.len() + 30;
            for file in &selected_files {
                total_len += file.len() + 3;
            }

            if total_len >= 8000 {
                match write_selection_to_temp_list(preset_name, &selected_files) {
                    Some(temp_file_path) => {
                        cmd.arg("--input-files").arg(&temp_file_path);
                    }
                    None => {
                        for file in &selected_files {
                            cmd.arg(file);
                        }
                    }
                }
            } else {
                for file in &selected_files {
                    cmd.arg(file);
                }
            }

            if cmd.spawn().is_ok() {
                Ok(())
            } else {
                dbg_log!("Failed to spawn converter process: {}", bin_path.display());
                Err(E_FAIL.into())
            }
        } else if verb_offset == presets_count {
            let bin_path = get_bin_path();
            if bin_path.exists() {
                match Command::new(&bin_path).arg("-settings").spawn() {
                    Ok(_) => Ok(()),
                    Err(e) => {
                        dbg_log!("Failed to open settings window: {:?}", e);
                        Err(E_FAIL.into())
                    }
                }
            } else {
                dbg_log!(
                    "Converter binary not found for settings: {}",
                    bin_path.display()
                );
                Err(E_FAIL.into())
            }
        } else {
            Ok(())
        }
    }

    fn GetCommandString(
        &self,
        _idcmd: usize,
        _utype: u32,
        _pwzreserved: *const u32,
        _pszname: PSTR,
        _cchmax: u32,
    ) -> Result<()> {
        Ok(())
    }
}

impl IShellPropSheetExt_Impl for FileConverterShellExt_Impl {
    fn AddPages(&self, _lpfnaddpage: LPFNADDPROPSHEETPAGE, _lparam: LPARAM) -> Result<()> {
        // Do not inject an unconfigured blank property sheet tab into Explorer
        Ok(())
    }

    fn ReplacePage(
        &self,
        _upageid: u32,
        _lpfnreplacepage: LPFNADDPROPSHEETPAGE,
        _lparam: LPARAM,
    ) -> Result<()> {
        Ok(())
    }
}

fn string_to_cotaskmem_pwstr(s: &str) -> Result<PWSTR> {
    let wide: Vec<u16> = s.encode_utf16().chain(std::iter::once(0)).collect();
    let bytes = wide.len() * std::mem::size_of::<u16>();
    unsafe {
        let ptr = CoTaskMemAlloc(bytes) as *mut u16;
        if ptr.is_null() {
            return Err(windows::Win32::Foundation::E_OUTOFMEMORY.into());
        }
        std::ptr::copy_nonoverlapping(wide.as_ptr(), ptr, wide.len());
        Ok(PWSTR(ptr))
    }
}

impl IExplorerCommand_Impl for FileConverterShellExt_Impl {
    fn GetTitle(&self, _psiitemarray: Option<&IShellItemArray>) -> Result<PWSTR> {
        string_to_cotaskmem_pwstr("File Converter")
    }

    fn GetIcon(&self, _psiitemarray: Option<&IShellItemArray>) -> Result<PWSTR> {
        let bin_path = get_bin_path();
        string_to_cotaskmem_pwstr(&bin_path.to_string_lossy())
    }

    fn GetToolTip(&self, _psiitemarray: Option<&IShellItemArray>) -> Result<PWSTR> {
        string_to_cotaskmem_pwstr("Convert files with File Converter")
    }

    fn GetCanonicalName(&self) -> Result<GUID> {
        Ok(CLSID_FILE_CONVERTER)
    }

    fn GetState(
        &self,
        psiitemarray: Option<&IShellItemArray>,
        _foktousecache: BOOL,
    ) -> Result<u32> {
        let mut has_files = false;
        if let Some(item_array) = psiitemarray {
            unsafe {
                if let Ok(count) = item_array.GetCount() {
                    has_files = count > 0;
                }
            }
        }
        if !has_files {
            if let Ok(lock) = self.selected_files.read() {
                has_files = !lock.is_empty();
            }
        }

        // Only advertise the verb when it can actually do something. Without this
        // the entry is offered as enabled and then silently does nothing.
        if has_files && !get_bin_path().exists() {
            dbg_log!("Converter binary not found; hiding context menu verb");
            has_files = false;
        }

        if has_files {
            Ok(ECS_ENABLED.0 as u32)
        } else {
            Ok(windows::Win32::UI::Shell::ECS_HIDDEN.0 as u32)
        }
    }

    fn Invoke(
        &self,
        psiitemarray: Option<&IShellItemArray>,
        _pbc: Option<&IBindCtx>,
    ) -> Result<()> {
        let mut files = Vec::new();
        if let Some(item_array) = psiitemarray {
            unsafe {
                if let Ok(count) = item_array.GetCount() {
                    for i in 0..count {
                        if let Ok(item) = item_array.GetItemAt(i) {
                            if let Ok(path_pwstr) = item.GetDisplayName(SIGDN_FILESYSPATH) {
                                if !path_pwstr.is_null() {
                                    let path_str = path_pwstr.to_string().unwrap_or_default();
                                    CoTaskMemFree(Some(path_pwstr.0 as *const c_void));
                                    if !path_str.is_empty() {
                                        files.push(path_str);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        if files.is_empty() {
            if let Ok(lock) = self.selected_files.read() {
                files = lock.clone();
            }
        }

        let bin_path = get_bin_path();
        if !bin_path.exists() {
            return Err(E_FAIL.into());
        }

        let mut cmd = Command::new(&bin_path);
        for file in files {
            cmd.arg(file);
        }
        // Report the real outcome: swallowing the spawn error made the verb look
        // successful while nothing happened at all.
        match cmd.spawn() {
            Ok(_) => Ok(()),
            Err(e) => {
                dbg_log!(
                    "Failed to spawn converter process {}: {:?}",
                    bin_path.display(),
                    e
                );
                Err(E_FAIL.into())
            }
        }
    }

    fn GetFlags(&self) -> Result<u32> {
        Ok(ECF_DEFAULT.0 as u32)
    }

    fn EnumSubCommands(&self) -> Result<IEnumExplorerCommand> {
        Err(windows::Win32::Foundation::E_NOTIMPL.into())
    }
}

fn create_default_settings() -> Settings {
    Settings::load_from_str(DEFAULT_SETTINGS_XML).unwrap_or_else(|_| Settings {
        serialization_version: 4,
        maximum_number_of_simultaneous_conversions: 2,
        exit_application_when_conversions_finished: true,
        duration_between_end_of_conversions_and_application_exit: 2.0,
        check_upgrade_at_startup: true,
        application_language_name: "en".to_string(),
        copy_files_in_clipboard_after_conversion: true,
        hardware_acceleration_mode: file_converter_core::types::HardwareAccelerationMode::Off,
        auto_start_on_file_drop: false,
        conversion_presets: vec![],
    })
}

/// Reads the module file path, truncating at the first NUL.
///
/// `GetModuleFileNameW` returns `nSize` on overflow, and `OsString::from_wide`
/// does **not** stop at interior NULs, so a truncated path would embed a U+0000
/// and make every downstream `exists()` check fail.
fn wide_to_path(buf: &[u16]) -> PathBuf {
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    PathBuf::from(OsString::from_wide(&buf[..len]))
}

fn get_bin_path() -> PathBuf {
    use winreg::RegKey;
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};

    if let Ok(hkcu) = RegKey::predef(HKEY_CURRENT_USER).open_subkey("Software\\FileConverter") {
        if let Ok(app_path) = hkcu.get_value::<String, _>("AppPath") {
            let path = PathBuf::from(&app_path);
            if path.exists() {
                return path;
            }
        }
    }

    if let Ok(hklm) = RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey("Software\\FileConverter") {
        if let Ok(app_path) = hklm.get_value::<String, _>("AppPath") {
            let path = PathBuf::from(&app_path);
            if path.exists() {
                return path;
            }
        }
    }

    let dll_hinst = G_DLL_INSTANCE.load(Ordering::Relaxed);
    if dll_hinst != 0 {
        let mut buf = vec![0u16; 512];
        let len = unsafe { GetModuleFileNameW(HMODULE(dll_hinst as *mut c_void), &mut buf) };
        if len > 0 {
            let dll_path = wide_to_path(&buf);
            if let Some(parent) = dll_path.parent() {
                let path = parent.join("file_converter_bin.exe");
                if path.exists() {
                    return path;
                }
            }
        }
    }

    if let Ok(mut exe_path) = std::env::current_exe() {
        exe_path.pop();
        let path = exe_path.join("file_converter_bin.exe");
        if path.exists() {
            return path;
        }
    }

    let local_app_data = std::env::var("LOCALAPPDATA").unwrap_or_default();
    let local_bin = Path::new(&local_app_data)
        .join("FileConverter")
        .join("file_converter_bin.exe");
    if local_bin.exists() {
        return local_bin;
    }

    let program_files = std::env::var("ProgramFiles").unwrap_or_default();
    let pf_bin = Path::new(&program_files)
        .join("FileConverter")
        .join("file_converter_bin.exe");
    if pf_bin.exists() {
        return pf_bin;
    }

    PathBuf::from("file_converter_bin.exe")
}

#[implement(IClassFactory)]
struct FileConverterClassFactory;

impl IClassFactory_Impl for FileConverterClassFactory_Impl {
    fn CreateInstance(
        &self,
        punkouter: Option<&windows::core::IUnknown>,
        riid: *const GUID,
        ppvobject: *mut *mut c_void,
    ) -> Result<()> {
        if punkouter.is_some() {
            return Err(CLASS_E_NOAGGREGATION.into());
        }

        // COM requires the out-parameter to be NULL on failure; `query` only
        // writes it on success, so clear it up front.
        if !ppvobject.is_null() {
            unsafe { *ppvobject = std::ptr::null_mut() };
        }

        let obj: IShellExtInit = FileConverterShellExt::new().into();
        unsafe { obj.query(riid, ppvobject).ok() }
    }

    fn LockServer(&self, flock: BOOL) -> Result<()> {
        if flock.as_bool() {
            G_LOCK_COUNT.fetch_add(1, Ordering::Relaxed);
        } else {
            // `fetch_sub` wraps: an unbalanced LockServer(FALSE) would pin the
            // count at u32::MAX and `DllCanUnloadNow` would return S_FALSE forever.
            let _ = G_LOCK_COUNT.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |v| {
                Some(v.saturating_sub(1))
            });
        }
        Ok(())
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn DllGetClassObject(
    rclsid: *const GUID,
    riid: *const GUID,
    ppv: *mut *mut c_void,
) -> HRESULT {
    if ppv.is_null() || rclsid.is_null() || riid.is_null() {
        return E_FAIL;
    }
    *ppv = std::ptr::null_mut();

    if *rclsid != CLSID_FILE_CONVERTER {
        return CLASS_E_CLASSNOTAVAILABLE;
    }

    let factory: IClassFactory = FileConverterClassFactory.into();
    factory.query(riid, ppv)
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn DllCanUnloadNow() -> HRESULT {
    if G_LOCK_COUNT.load(Ordering::Relaxed) == 0 && G_OBJECT_COUNT.load(Ordering::Relaxed) == 0 {
        S_OK
    } else {
        S_FALSE
    }
}

#[cfg(target_os = "windows")]
#[unsafe(no_mangle)]
pub unsafe extern "system" fn DllRegisterServer() -> HRESULT {
    use winreg::RegKey;
    use winreg::enums::{HKEY_CLASSES_ROOT, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_WRITE};

    let hmodule = G_DLL_INSTANCE.load(Ordering::Relaxed);
    let mut module_path = PathBuf::new();

    if hmodule != 0 {
        let mut buf = vec![0u16; 512];
        let len = GetModuleFileNameW(HMODULE(hmodule as *mut c_void), &mut buf);
        if len > 0 {
            module_path = wide_to_path(&buf);
        }
    }

    if module_path.as_os_str().is_empty() {
        let dll_name: Vec<u16> = "file_converter_shell.dll\0".encode_utf16().collect();
        if let Ok(h) = GetModuleHandleW(windows::core::PCWSTR(dll_name.as_ptr())) {
            let mut buf = vec![0u16; 512];
            let len = GetModuleFileNameW(h, &mut buf);
            if len > 0 {
                module_path = wide_to_path(&buf);
            }
        }
    }

    let mod_path_str = module_path.to_string_lossy().to_string();

    if !mod_path_str.is_empty() {
        if let Some(parent) = module_path.parent() {
            let bin_exe = parent.join("file_converter_bin.exe");
            let bin_path_str = bin_exe.to_string_lossy().to_string();

            if let Ok((hkcu, _)) =
                RegKey::predef(HKEY_CURRENT_USER).create_subkey("Software\\FileConverter")
            {
                let _ = hkcu.set_value("AppPath", &bin_path_str);
            }
            if let Ok((hklm, _)) =
                RegKey::predef(HKEY_LOCAL_MACHINE).create_subkey("Software\\FileConverter")
            {
                let _ = hklm.set_value("AppPath", &bin_path_str);
            }
        }
    }

    let clsid_str = "{AF9B72B5-F4E4-44B0-A3D9-B55B748EFE90}";
    let hkcr = RegKey::predef(HKEY_CLASSES_ROOT);
    let hklm_classes =
        RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey_with_flags("Software\\Classes", KEY_WRITE);
    let hkcu_classes = RegKey::predef(HKEY_CURRENT_USER)
        .create_subkey("Software\\Classes")
        .map(|(k, _)| k);

    let clsid_key_path = format!("CLSID\\{}", clsid_str);
    let clsid_inproc_path = format!("CLSID\\{}\\InprocServer32", clsid_str);

    // A local mutable flag rather than discarding every write error: regsvr32
    // previously reported success even when nothing could be written.
    let mut write_failed = false;

    let _ = hkcr.delete_subkey_all(&clsid_key_path);
    if let Ok(ref root) = hklm_classes {
        let _ = root.delete_subkey_all(&clsid_key_path);
    }
    if let Ok(ref root) = hkcu_classes {
        let _ = root.delete_subkey_all(&clsid_key_path);
    }
    // Target HKLM\Software\Classes if writable (system-wide), otherwise fall back to
    // HKCU\Software\Classes (per-user).
    let root_classes = if let Ok(ref root) = hklm_classes {
        root
    } else if let Ok(ref root) = hkcu_classes {
        root
    } else {
        write_failed = true;
        &hkcr
    };

    if mod_path_str.is_empty() {
        // Without the DLL's own path there is nothing meaningful to register, and
        // writing a *relative* `command` value would make Explorer try to launch
        // `%WINDIR%\file_converter_bin.exe`.
        dbg_log!("Module path unknown; aborting registration");
        return E_FAIL;
    }

    match root_classes.create_subkey(&clsid_key_path) {
        Ok((key, _)) => {
            if key.set_value("", &"FileConverter Shell Extension").is_err() {
                write_failed = true;
            }
        }
        Err(e) => {
            dbg_log!("Failed to create {}: {:?}", clsid_key_path, e);
            write_failed = true;
        }
    }
    match root_classes.create_subkey(&clsid_inproc_path) {
        Ok((key, _)) => {
            if key.set_value("", &mod_path_str).is_err()
                || key.set_value("ThreadingModel", &"Apartment").is_err()
            {
                write_failed = true;
            }
        }
        Err(e) => {
            dbg_log!("Failed to create {}: {:?}", clsid_inproc_path, e);
            write_failed = true;
        }
    }

    // Register handlers on files and all filesystem objects. Folders are
    // deliberately not registered: they have no extension, so every preset would
    // look applicable and the menu would offer conversions that cannot run.
    let associations = ["*", "AllFilesystemObjects"];

    let Some(parent) = module_path.parent() else {
        dbg_log!("Module path has no parent directory; aborting registration");
        return E_FAIL;
    };
    let bin_exe = parent.join("file_converter_bin.exe");
    let bin_exe_str = bin_exe.to_string_lossy().to_string();

    for assoc in &associations {
        let path = format!("{}\\shellex\\ContextMenuHandlers\\FileConverter", assoc);
        match root_classes.create_subkey(&path) {
            Ok((key, _)) => {
                if key.set_value("", &clsid_str).is_err() {
                    write_failed = true;
                }
            }
            Err(e) => {
                dbg_log!("Failed to create {}: {:?}", path, e);
                write_failed = true;
            }
        }

        let prop_path = format!("{}\\shellex\\PropertySheetHandlers\\FileConverter", assoc);
        match root_classes.create_subkey(&prop_path) {
            Ok((key, _)) => {
                if key.set_value("", &clsid_str).is_err() {
                    write_failed = true;
                }
            }
            Err(e) => {
                dbg_log!("Failed to create {}: {:?}", prop_path, e);
                write_failed = true;
            }
        }

        // Modern Windows 11 Explorer Command & Shell Verb Handler
        let shell_verb_path = format!("{}\\shell\\FileConverter", assoc);
        match root_classes.create_subkey(&shell_verb_path) {
            Ok((key, _)) => {
                if key.set_value("", &"File Converter").is_err()
                    || key.set_value("MUIVerb", &"File Converter").is_err()
                    || key.set_value("Icon", &bin_exe_str).is_err()
                    || key.set_value("ExplorerCommandHandler", &clsid_str).is_err()
                {
                    write_failed = true;
                }
            }
            Err(e) => {
                dbg_log!("Failed to create {}: {:?}", shell_verb_path, e);
                write_failed = true;
            }
        }
        let shell_verb_cmd_path = format!("{}\\shell\\FileConverter\\command", assoc);
        match root_classes.create_subkey(&shell_verb_cmd_path) {
            Ok((key, _)) => {
                let cmd_str = format!("\"{}\" \"%1\"", bin_exe_str);
                if key.set_value("", &cmd_str).is_err() {
                    write_failed = true;
                }
            }
            Err(e) => {
                dbg_log!("Failed to create {}: {:?}", shell_verb_cmd_path, e);
                write_failed = true;
            }
        }
    }

    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    if let Ok((key, _)) = hklm
        .create_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Shell Extensions\\Approved")
        && key
            .set_value(clsid_str, &"File Converter Context Menu Handler")
            .is_err()
    {
        // Non-elevated registration cannot write HKLM; that is expected and not a
        // hard failure because the per-user CLSID registration above still works.
        dbg_log!("Could not write HKLM Approved entry (needs elevation)");
    }

    SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None);

    if write_failed {
        dbg_log!("Shell extension registration completed with errors");
        return E_FAIL;
    }

    S_OK
}

#[cfg(target_os = "windows")]
#[unsafe(no_mangle)]
pub unsafe extern "system" fn DllUnregisterServer() -> HRESULT {
    use winreg::RegKey;
    use winreg::enums::{HKEY_CLASSES_ROOT, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_ALL_ACCESS};

    let clsid_str = "{AF9B72B5-F4E4-44B0-A3D9-B55B748EFE90}";
    let hkcr = RegKey::predef(HKEY_CLASSES_ROOT);
    let hklm_classes = RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey_with_flags("Software\\Classes", KEY_ALL_ACCESS);
    let hkcu_classes = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey_with_flags("Software\\Classes", KEY_ALL_ACCESS);

    let clsid_key_path = format!("CLSID\\{}", clsid_str);
    let _ = hkcr.delete_subkey_all(&clsid_key_path);
    if let Ok(ref root) = hklm_classes {
        let _ = root.delete_subkey_all(&clsid_key_path);
    }
    if let Ok(ref root) = hkcu_classes {
        let _ = root.delete_subkey_all(&clsid_key_path);
    }

    let associations = [
        "*",
        "AllFilesystemObjects",
        "Directory",
        "Directory\\Background",
        "Drive",
        "Folder",
    ];

    for assoc in &associations {
        let path = format!("{}\\shellex\\ContextMenuHandlers\\FileConverter", assoc);
        let _ = hkcr.delete_subkey_all(&path);
        if let Ok(ref root) = hklm_classes {
            let _ = root.delete_subkey_all(&path);
        }
        if let Ok(ref root) = hkcu_classes {
            let _ = root.delete_subkey_all(&path);
        }

        let prop_path = format!("{}\\shellex\\PropertySheetHandlers\\FileConverter", assoc);
        let _ = hkcr.delete_subkey_all(&prop_path);
        if let Ok(ref root) = hklm_classes {
            let _ = root.delete_subkey_all(&prop_path);
        }
        if let Ok(ref root) = hkcu_classes {
            let _ = root.delete_subkey_all(&prop_path);
        }

        let verb_path = format!("{}\\shell\\FileConverter", assoc);
        let _ = hkcr.delete_subkey_all(&verb_path);
        if let Ok(ref root) = hklm_classes {
            let _ = root.delete_subkey_all(&verb_path);
        }
        if let Ok(ref root) = hkcu_classes {
            let _ = root.delete_subkey_all(&verb_path);
        }
    }

    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    if let Ok(key) =
        hklm.open_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Shell Extensions\\Approved")
    {
        let _ = key.delete_value(clsid_str);
    }

    SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None);

    S_OK
}

#[cfg(test)]
mod tests {
    use super::*;
    use file_converter_core::types::InputPostConversionAction;

    #[test]
    fn test_shell_preset_compatibility_filtering() {
        let preset = ConversionPreset {
            name: "To MP3".to_string(),
            output_type: OutputType::Mp3,
            output_file_name_template: "(p)\\(f)".to_string(),
            is_default_settings: true,
            input_types: vec!["flac".into(), "wav".into()],
            input_post_conversion_action: InputPostConversionAction::None,
            settings: vec![],
        };

        assert!(is_preset_compatible_with_file(
            &preset,
            "C:\\Music\\song.flac"
        ));
        assert!(is_preset_compatible_with_file(
            &preset,
            "C:\\Music\\audio.wav"
        ));
        assert!(!is_preset_compatible_with_file(
            &preset,
            "C:\\Photos\\image.png"
        ));
    }

    #[test]
    fn test_shell_category_icon_creation() {
        let hbmp_audio = create_category_icon(OutputType::Mp3);
        assert!(!hbmp_audio.is_invalid());

        let hbmp_video = create_category_icon(OutputType::Mp4);
        assert!(!hbmp_video.is_invalid());

        let hbmp_image = create_category_icon(OutputType::Png);
        assert!(!hbmp_image.is_invalid());

        let hbmp_pdf = create_category_icon(OutputType::Pdf);
        assert!(!hbmp_pdf.is_invalid());
    }
}
