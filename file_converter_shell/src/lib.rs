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
use file_converter_core::types::{
    OutputType, get_extension_category, is_output_type_compatible_with_category,
};

#[allow(unused_imports)]
mod windows_core {
    pub use windows::core::*;
}
use windows::Win32::Foundation::{
    BOOL, CLASS_E_CLASSNOTAVAILABLE, CLASS_E_NOAGGREGATION, E_FAIL, HINSTANCE, HMODULE, HWND,
    LPARAM, S_FALSE, S_OK, WPARAM,
};
use windows::Win32::Graphics::Gdi::{CreateBitmap, HBITMAP};
use windows::Win32::System::Com::{
    DVASPECT_CONTENT, FORMATETC, IClassFactory, IClassFactory_Impl, IDataObject, STGMEDIUM,
    TYMED_HGLOBAL,
};
use windows::core::{GUID, HRESULT, Interface, PSTR, Result, implement};

#[link(name = "ole32")]
unsafe extern "system" {
    fn ReleaseStgMedium(pmedium: *mut STGMEDIUM);
}
use windows::Win32::System::LibraryLoader::{GetModuleFileNameW, GetModuleHandleW};
use windows::Win32::System::Memory::{GlobalLock, GlobalUnlock};
use windows::Win32::System::Registry::HKEY;
use windows::Win32::UI::Controls::{
    CreatePropertySheetPageW, HPROPSHEETPAGE, PROPSHEETPAGEW, PSP_DEFAULT,
};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    CMINVOKECOMMANDINFO, DragQueryFileW, HDROP, IContextMenu, IContextMenu_Impl, IShellExtInit,
    IShellExtInit_Impl, IShellPropSheetExt, IShellPropSheetExt_Impl, SHCNE_ASSOCCHANGED,
    SHCNF_IDLIST, SHChangeNotify,
};

type LPFNADDPROPSHEETPAGE = Option<unsafe extern "system" fn(HPROPSHEETPAGE, LPARAM) -> BOOL>;
use windows::Win32::UI::WindowsAndMessaging::{
    CreatePopupMenu, HMENU, InsertMenuItemW, MENUITEMINFOW, MFT_SEPARATOR, MFT_STRING, MIIM_BITMAP,
    MIIM_FTYPE, MIIM_ID, MIIM_STRING, MIIM_SUBMENU,
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
    }
    1
}

unsafe fn create_category_icon(output_type: OutputType) -> HBITMAP {
    let color: u32 = match output_type {
        OutputType::Aac
        | OutputType::Flac
        | OutputType::Mp3
        | OutputType::Ogg
        | OutputType::Wav => 0x00D09000,
        OutputType::Avi
        | OutputType::Mkv
        | OutputType::Mp4
        | OutputType::Ogv
        | OutputType::Webm => 0x003030E0,
        OutputType::Avif
        | OutputType::Ico
        | OutputType::Jpg
        | OutputType::Png
        | OutputType::Webp
        | OutputType::Gif => 0x0040A040,
        OutputType::Pdf => 0x001080E0,
        _ => 0x00808080,
    };

    let mut pixels = [color; 16 * 16];
    for y in 0..16 {
        for x in 0..16 {
            if x == 0 || x == 15 || y == 0 || y == 15 {
                pixels[y * 16 + x] = 0x00303030;
            }
        }
    }
    CreateBitmap(16, 16, 1, 32, Some(pixels.as_ptr() as *const c_void))
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
    unsafe {
        if let Ok(mut medium) = data_obj.GetData(&fmt) {
            let h_drop = medium.u.hGlobal;
            if !h_drop.0.is_null() {
                let ptr = GlobalLock(h_drop);
                if !ptr.is_null() {
                    let file_count = DragQueryFileW(HDROP(ptr), 0xFFFFFFFF, None);
                    for i in 0..file_count {
                        let size = DragQueryFileW(HDROP(ptr), i, None);
                        if size > 0 {
                            let mut buf = vec![0u16; (size + 1) as usize];
                            DragQueryFileW(HDROP(ptr), i, Some(&mut buf));
                            if let Some(null_pos) = buf.iter().position(|&x| x == 0) {
                                let os_str = OsString::from_wide(&buf[..null_pos]);
                                if let Ok(path_str) = os_str.into_string() {
                                    files.push(path_str);
                                }
                            }
                        }
                    }
                    let _ = GlobalUnlock(h_drop);
                }
            }
            ReleaseStgMedium(&mut medium);
        }
    }
    files
}

#[implement(IShellExtInit, IContextMenu, IShellPropSheetExt)]
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
    let ext = Path::new(file_path)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();

    if ext.is_empty() {
        return true;
    }

    if !preset.input_types.is_empty() {
        if preset.input_types.iter().any(|it| {
            let clean_it = it.trim().trim_start_matches('.').to_lowercase();
            clean_it == "*" || clean_it == ext
        }) {
            return true;
        }
    }

    let cat = get_extension_category(&ext);
    is_output_type_compatible_with_category(preset.output_type, cat)
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
        let cmd_id = idcmdfirst;
        let configure_cmd_id = cmd_id + presets_count as u32;

        let mut parent_text_wide: Vec<u16> = "File Converter\0".encode_utf16().collect();

        unsafe {
            if presets_count <= 5 {
                for (i, preset) in compatible_presets.iter().enumerate() {
                    let mut name_wide: Vec<u16> = preset.name.encode_utf16().collect();
                    name_wide.push(0);

                    let icon_bmp = create_category_icon(preset.output_type);

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

                    let _ = InsertMenuItemW(hmenu, indexmenu + i as u32, true, &mii);
                }

                HRESULT(presets_count as i32).ok()
            } else {
                let h_sub_menu = CreatePopupMenu()?;

                for (i, preset) in compatible_presets.iter().enumerate() {
                    let mut name_wide: Vec<u16> = preset.name.encode_utf16().collect();
                    name_wide.push(0);

                    let icon_bmp = create_category_icon(preset.output_type);

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

                    let _ = InsertMenuItemW(h_sub_menu, i as u32, true, &mii);
                }

                let sep_mii = MENUITEMINFOW {
                    cbSize: std::mem::size_of::<MENUITEMINFOW>() as u32,
                    fMask: MIIM_FTYPE,
                    fType: MFT_SEPARATOR,
                    ..Default::default()
                };
                let _ = InsertMenuItemW(h_sub_menu, presets_count as u32, true, &sep_mii);

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
                let _ = InsertMenuItemW(h_sub_menu, (presets_count + 1) as u32, true, &config_mii);

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

                let _ = InsertMenuItemW(hmenu, indexmenu, true, &parent_mii);
                HRESULT((presets_count + 2) as i32).ok()
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
                    selected_files
                        .iter()
                        .all(|file| is_preset_compatible_with_file(preset, file))
                })
                .map(|p| p.name)
                .collect();
        }

        let presets_count = presets.len();

        if verb_offset < presets_count {
            let preset_name = &presets[verb_offset];

            let bin_path = get_bin_path();
            if !bin_path.exists() {
                return Err(E_FAIL.into());
            }

            let mut cmd = Command::new(&bin_path);
            cmd.arg("--conversion-preset").arg(preset_name);

            let mut total_len = preset_name.len() + 30;
            for file in &selected_files {
                total_len += file.len() + 3;
            }

            if total_len >= 8000 {
                let temp_dir = std::env::temp_dir();
                let pid = std::process::id();
                let temp_file_path =
                    temp_dir.join(format!("file-converter-input-list-{}.txt", pid));

                if let Ok(mut file) = std::fs::File::create(&temp_file_path) {
                    use std::io::Write;
                    for path in &selected_files {
                        let _ = writeln!(file, "{}", path);
                    }
                    cmd.arg("--input-files").arg(&temp_file_path);
                } else {
                    for file in &selected_files {
                        cmd.arg(file);
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
                Err(E_FAIL.into())
            }
        } else if verb_offset == presets_count {
            let bin_path = get_bin_path();
            if bin_path.exists() {
                let _ = Command::new(&bin_path).arg("-settings").spawn();
                Ok(())
            } else {
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
    fn AddPages(&self, lpfnaddpage: LPFNADDPROPSHEETPAGE, lparam: LPARAM) -> Result<()> {
        if let Some(add_page_fn) = lpfnaddpage {
            let mut psp = PROPSHEETPAGEW {
                dwSize: std::mem::size_of::<PROPSHEETPAGEW>() as u32,
                dwFlags: PSP_DEFAULT,
                hInstance: HINSTANCE(G_DLL_INSTANCE.load(Ordering::Relaxed) as *mut c_void),
                pfnDlgProc: Some(file_converter_prop_page_proc),
                ..Default::default()
            };
            unsafe {
                let hpage = CreatePropertySheetPageW(&mut psp);
                if !hpage.is_invalid() {
                    let _ = add_page_fn(hpage, lparam);
                }
            }
        }
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

unsafe extern "system" fn file_converter_prop_page_proc(
    _hwnd: HWND,
    _msg: u32,
    _wparam: WPARAM,
    _lparam: LPARAM,
) -> isize {
    0
}

fn create_default_settings() -> Settings {
    Settings {
        serialization_version: 4,
        maximum_number_of_simultaneous_conversions: 2,
        exit_application_when_conversions_finished: true,
        duration_between_end_of_conversions_and_application_exit: 2.0,
        check_upgrade_at_startup: true,
        application_language_name: "en".to_string(),
        copy_files_in_clipboard_after_conversion: true,
        hardware_acceleration_mode: file_converter_core::types::HardwareAccelerationMode::Off,
        conversion_presets: vec![],
    }
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
            let os_str = OsString::from_wide(&buf[..len as usize]);
            let dll_path = PathBuf::from(os_str);
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

        let obj: IShellExtInit = FileConverterShellExt::new().into();
        unsafe { obj.query(riid, ppvobject).ok() }
    }

    fn LockServer(&self, flock: BOOL) -> Result<()> {
        if flock.as_bool() {
            G_LOCK_COUNT.fetch_add(1, Ordering::Relaxed);
        } else {
            G_LOCK_COUNT.fetch_sub(1, Ordering::Relaxed);
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
    use winreg::enums::{HKEY_CLASSES_ROOT, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_ALL_ACCESS};

    let hmodule = G_DLL_INSTANCE.load(Ordering::Relaxed);
    let mut module_path = PathBuf::new();

    if hmodule != 0 {
        let mut buf = vec![0u16; 512];
        let len = GetModuleFileNameW(HMODULE(hmodule as *mut c_void), &mut buf);
        if len > 0 {
            let os_str = OsString::from_wide(&buf[..len as usize]);
            module_path = PathBuf::from(os_str);
        }
    }

    if module_path.as_os_str().is_empty() {
        let dll_name: Vec<u16> = "file_converter_shell.dll\0".encode_utf16().collect();
        if let Ok(h) = GetModuleHandleW(windows::core::PCWSTR(dll_name.as_ptr())) {
            let mut buf = vec![0u16; 512];
            let len = GetModuleFileNameW(h, &mut buf);
            if len > 0 {
                let os_str = OsString::from_wide(&buf[..len as usize]);
                module_path = PathBuf::from(os_str);
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
    let hklm_classes = RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey_with_flags("Software\\Classes", KEY_ALL_ACCESS);
    let hkcu_classes = RegKey::predef(HKEY_CURRENT_USER)
        .create_subkey("Software\\Classes")
        .map(|(k, _)| k);

    let clsid_key_path = format!("CLSID\\{}", clsid_str);
    let clsid_inproc_path = format!("CLSID\\{}\\InprocServer32", clsid_str);

    let _ = hkcr.delete_subkey_all(&clsid_key_path);
    if let Ok(ref root) = hklm_classes {
        let _ = root.delete_subkey_all(&clsid_key_path);
    }
    if let Ok(ref root) = hkcu_classes {
        let _ = root.delete_subkey_all(&clsid_key_path);
    }

    if let Ok(ref root) = hkcu_classes {
        if let Ok((key, _)) = root.create_subkey(&clsid_key_path) {
            let _ = key.set_value("", &"FileConverter Shell Extension");
        }
        if let Ok((key, _)) = root.create_subkey(&clsid_inproc_path) {
            let _ = key.set_value("", &mod_path_str);
            let _ = key.set_value("ThreadingModel", &"Apartment");
        }
    }

    if let Ok((key, _)) = hkcr.create_subkey(&clsid_key_path) {
        let _ = key.set_value("", &"FileConverter Shell Extension");
    }
    if let Ok((key, _)) = hkcr.create_subkey(&clsid_inproc_path) {
        let _ = key.set_value("", &mod_path_str);
        let _ = key.set_value("ThreadingModel", &"Apartment");
    }
    if let Ok(ref root) = hklm_classes {
        if let Ok((key, _)) = root.create_subkey(&clsid_key_path) {
            let _ = key.set_value("", &"FileConverter Shell Extension");
        }
        if let Ok((key, _)) = root.create_subkey(&clsid_inproc_path) {
            let _ = key.set_value("", &mod_path_str);
            let _ = key.set_value("ThreadingModel", &"Apartment");
        }
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

        if let Ok(ref root) = hkcu_classes {
            let _ = root.delete_subkey_all(&path);
            if let Ok((key, _)) = root.create_subkey(&path) {
                let _ = key.set_value("", &clsid_str);
            }
        }

        if let Ok((key, _)) = hkcr.create_subkey(&path) {
            let _ = key.set_value("", &clsid_str);
        }
        if let Ok(ref root) = hklm_classes {
            if let Ok((key, _)) = root.create_subkey(&path) {
                let _ = key.set_value("", &clsid_str);
            }
        }

        let prop_path = format!("{}\\shellex\\PropertySheetHandlers\\FileConverter", assoc);
        if let Ok(ref root) = hkcu_classes {
            let _ = root.delete_subkey_all(&prop_path);
            if let Ok((key, _)) = root.create_subkey(&prop_path) {
                let _ = key.set_value("", &clsid_str);
            }
        }
        if let Ok((key, _)) = hkcr.create_subkey(&prop_path) {
            let _ = key.set_value("", &clsid_str);
        }
        if let Ok(ref root) = hklm_classes {
            if let Ok((key, _)) = root.create_subkey(&prop_path) {
                let _ = key.set_value("", &clsid_str);
            }
        }

        let bin_exe = if let Some(parent) = module_path.parent() {
            parent.join("file_converter_bin.exe")
        } else {
            PathBuf::from("file_converter_bin.exe")
        };
        let shell_verb_path = format!("{}\\shell\\FileConverter", assoc);
        let shell_verb_cmd_path = format!("{}\\shell\\FileConverter\\command", assoc);
        let cmd_str = format!("\"{}\" -settings", bin_exe.to_string_lossy());

        if let Ok(ref root) = hkcu_classes {
            if let Ok((key, _)) = root.create_subkey(&shell_verb_path) {
                let _ = key.set_value("", &"File Converter");
                let _ = key.set_value("MUIVerb", &"File Converter");
            }
            if let Ok((key, _)) = root.create_subkey(&shell_verb_cmd_path) {
                let _ = key.set_value("", &cmd_str);
            }
        }
        if let Ok((key, _)) = hkcr.create_subkey(&shell_verb_path) {
            let _ = key.set_value("", &"File Converter");
            let _ = key.set_value("MUIVerb", &"File Converter");
        }
        if let Ok((key, _)) = hkcr.create_subkey(&shell_verb_cmd_path) {
            let _ = key.set_value("", &cmd_str);
        }
    }

    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    if let Ok((key, _)) = hklm
        .create_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Shell Extensions\\Approved")
    {
        let _ = key.set_value(clsid_str, &"File Converter Context Menu Handler");
    }

    SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None);

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
