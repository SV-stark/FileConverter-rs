#![allow(
    non_snake_case,
    non_camel_case_types,
    clippy::missing_safety_doc,
    clippy::all,
    warnings
)]

use std::ffi::{c_void, OsString};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::{LazyLock, RwLock};
use std::time::SystemTime;

use file_converter_core::settings::{ConversionPreset, Settings};
use file_converter_core::types::OutputType;

// Standard Win32 Types and Constants
type HRESULT = i32;
type ULONG = u32;
type HMENU = *mut c_void;

const S_OK: HRESULT = 0;
const S_FALSE: HRESULT = 1;
const E_NOINTERFACE: HRESULT = -2147467262; // 0x80004002
const E_OUTOFMEMORY: HRESULT = -2147024882; // 0x8007000E
const E_FAIL: HRESULT = -2147467259; // 0x80004005

const CF_HDROP: u32 = 15;
const DVASPECT_CONTENT: u32 = 1;
const TYMED_HGLOBAL: u32 = 1;

const MIIM_STRING: u32 = 64;
const MIIM_SUBMENU: u32 = 4;
const MIIM_FTYPE: u32 = 256;
const MIIM_ID: u32 = 2;

const MFT_STRING: u32 = 0;
const MFT_SEPARATOR: u32 = 2048;

// GUID struct representation
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq)]
struct GUID {
    data1: u32,
    data2: u16,
    data3: u16,
    data4: [u8; 8],
}

impl GUID {
    const CLSID_FILE_CONVERTER: GUID = GUID {
        data1: 0xAF9B72B5,
        data2: 0xF4E4,
        data3: 0x44B0,
        data4: [0xA3, 0xD9, 0xB5, 0x5B, 0x74, 0x8E, 0xFE, 0x90],
    };

    const IID_IUNKNOWN: GUID = GUID {
        data1: 0x00000000,
        data2: 0x0000,
        data3: 0x0000,
        data4: [0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46],
    };

    const IID_ICLASSFACTORY: GUID = GUID {
        data1: 0x00000001,
        data2: 0x0000,
        data3: 0x0000,
        data4: [0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46],
    };

    const IID_ISHELLEXTINIT: GUID = GUID {
        data1: 0x000214E8,
        data2: 0x0000,
        data3: 0x0000,
        data4: [0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46],
    };

    const IID_ICONTEXTMENU: GUID = GUID {
        data1: 0x000214E4,
        data2: 0x0000,
        data3: 0x0000,
        data4: [0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46],
    };
}

// Struct layouts for Win32 API
#[repr(C)]
struct FORMATETC {
    cfFormat: u32,
    ptd: *mut c_void,
    dwAspect: u32,
    lindex: i32,
    tymed: u32,
}

#[repr(C)]
struct STGMEDIUM {
    tymed: u32,
    hGlobal: *mut c_void,
    pUnkForRelease: *mut c_void,
}

#[repr(C)]
struct CMINVOKECOMMANDINFO {
    cbSize: u32,
    fMask: u32,
    hwnd: *mut c_void,
    lpVerb: *const u8,
    lpParameters: *const u8,
    lpDirectory: *const u8,
    nShow: i32,
    dwHotKey: u32,
    hIcon: *mut c_void,
}

#[repr(C)]
struct MENUITEMINFOW {
    cbSize: u32,
    fMask: u32,
    fType: u32,
    fState: u32,
    wID: u32,
    hSubMenu: HMENU,
    hbmpChecked: *mut c_void,
    hbmpUnchecked: *mut c_void,
    dwItemData: usize,
    dwTypeData: *mut u16,
    cch: u32,
    hbmpItem: *mut c_void,
}

// Windows APIs imports
unsafe extern "system" {
    fn GlobalLock(hMem: *mut c_void) -> *mut c_void;
    fn GlobalUnlock(hMem: *mut c_void) -> i32;
    fn DragQueryFileW(hDrop: *mut c_void, iFile: u32, lpszFile: *mut u16, cch: u32) -> u32;
    fn ReleaseStgMedium(pmedium: *mut STGMEDIUM);

    fn CreatePopupMenu() -> HMENU;
    fn InsertMenuItemW(
        hMenu: HMENU,
        uItem: u32,
        fByPosition: i32,
        lpmii: *const MENUITEMINFOW,
    ) -> i32;

    fn GetModuleFileNameW(hModule: *mut c_void, lpFilename: *mut u16, nSize: u32) -> u32;
    fn GetModuleHandleW(lpModuleName: *const u16) -> *mut c_void;
}

// Atomic DLL instance handle & active COM object counters
static G_DLL_INSTANCE: AtomicUsize = AtomicUsize::new(0);
static G_LOCK_COUNT: AtomicU32 = AtomicU32::new(0);
static G_OBJECT_COUNT: AtomicU32 = AtomicU32::new(0);

#[no_mangle]
pub unsafe extern "system" fn DllMain(
    hinst_dll: *mut c_void,
    fdw_reason: u32,
    _lpv_reserved: *mut c_void,
) -> i32 {
    if fdw_reason == 1 {
        // DLL_PROCESS_ATTACH
        G_DLL_INSTANCE.store(hinst_dll as usize, Ordering::Relaxed);
    }
    1
}

// Interface VTables
#[repr(C)]
struct IUnknownVtbl {
    QueryInterface: unsafe extern "system" fn(
        this: *mut c_void,
        riid: *const GUID,
        ppv: *mut *mut c_void,
    ) -> HRESULT,
    AddRef: unsafe extern "system" fn(this: *mut c_void) -> ULONG,
    Release: unsafe extern "system" fn(this: *mut c_void) -> ULONG,
}

#[repr(C)]
struct IClassFactoryVtbl {
    QueryInterface: unsafe extern "system" fn(
        this: *mut c_void,
        riid: *const GUID,
        ppv: *mut *mut c_void,
    ) -> HRESULT,
    AddRef: unsafe extern "system" fn(this: *mut c_void) -> ULONG,
    Release: unsafe extern "system" fn(this: *mut c_void) -> ULONG,
    CreateInstance: unsafe extern "system" fn(
        this: *mut c_void,
        pUnkOuter: *mut c_void,
        riid: *const GUID,
        ppvObject: *mut *mut c_void,
    ) -> HRESULT,
    LockServer: unsafe extern "system" fn(this: *mut c_void, fLock: i32) -> HRESULT,
}

#[repr(C)]
struct IShellExtInitVtbl {
    QueryInterface: unsafe extern "system" fn(
        this: *mut c_void,
        riid: *const GUID,
        ppv: *mut *mut c_void,
    ) -> HRESULT,
    AddRef: unsafe extern "system" fn(this: *mut c_void) -> ULONG,
    Release: unsafe extern "system" fn(this: *mut c_void) -> ULONG,
    Initialize: unsafe extern "system" fn(
        this: *mut c_void,
        pidlFolder: *const c_void,
        pDataObj: *mut IDataObject,
        hkeyProgID: *mut c_void,
    ) -> HRESULT,
}

#[repr(C)]
struct IContextMenuVtbl {
    QueryInterface: unsafe extern "system" fn(
        this: *mut c_void,
        riid: *const GUID,
        ppv: *mut *mut c_void,
    ) -> HRESULT,
    AddRef: unsafe extern "system" fn(this: *mut c_void) -> ULONG,
    Release: unsafe extern "system" fn(this: *mut c_void) -> ULONG,
    QueryContextMenu: unsafe extern "system" fn(
        this: *mut c_void,
        hmenu: HMENU,
        indexMenu: u32,
        idCmdFirst: u32,
        idCmdLast: u32,
        uFlags: u32,
    ) -> HRESULT,
    InvokeCommand:
        unsafe extern "system" fn(this: *mut c_void, pici: *const CMINVOKECOMMANDINFO) -> HRESULT,
    GetCommandString: unsafe extern "system" fn(
        this: *mut c_void,
        idCmd: usize,
        uFlags: u32,
        pwzReserved: *mut u32,
        pszName: *mut u8,
        cchMax: u32,
    ) -> HRESULT,
}

#[repr(C)]
struct IDataObject {
    lpVtbl: *const IDataObjectVtbl,
}

#[repr(C)]
struct IDataObjectVtbl {
    QueryInterface: unsafe extern "system" fn(
        this: *mut IDataObject,
        riid: *const GUID,
        ppv: *mut *mut c_void,
    ) -> HRESULT,
    AddRef: unsafe extern "system" fn(this: *mut IDataObject) -> ULONG,
    Release: unsafe extern "system" fn(this: *mut IDataObject) -> ULONG,
    GetData: unsafe extern "system" fn(
        this: *mut IDataObject,
        pformatetc: *const FORMATETC,
        pmedium: *mut STGMEDIUM,
    ) -> HRESULT,
}

// COM Struct Implementation
#[repr(C)]
struct FileConverterShell {
    context_menu_vtbl: *const IContextMenuVtbl,
    shell_ext_vtbl: *const IShellExtInitVtbl,
    ref_count: AtomicU32,
    selected_files: Vec<String>,
    /// Preset names in the order their menu IDs were assigned during QueryContextMenu.
    /// Index == (lpVerb offset from idCmdFirst) for a conversion item.
    active_presets: Vec<String>,
    /// The lpVerb offset that maps to the "Configure..." item (only valid when a
    /// submenu was built, i.e. more than 5 presets).
    configure_cmd_offset: Option<usize>,
}

static CONTEXT_MENU_VTBL: IContextMenuVtbl = IContextMenuVtbl {
    QueryInterface: FileConverterShell_QueryInterface_ContextMenu,
    AddRef: FileConverterShell_AddRef,
    Release: FileConverterShell_Release,
    QueryContextMenu: FileConverterShell_QueryContextMenu,
    InvokeCommand: FileConverterShell_InvokeCommand,
    GetCommandString: FileConverterShell_GetCommandString,
};

static SHELL_EXT_VTBL: IShellExtInitVtbl = IShellExtInitVtbl {
    QueryInterface: FileConverterShell_QueryInterface_ShellExt,
    AddRef: ShellExt_AddRef,
    Release: ShellExt_Release,
    Initialize: FileConverterShell_Initialize,
};

unsafe extern "system" fn ShellExt_AddRef(this: *mut c_void) -> ULONG {
    let offset = std::mem::offset_of!(FileConverterShell, shell_ext_vtbl);
    let obj = (this as usize - offset) as *mut FileConverterShell;
    FileConverterShell_AddRef(obj as *mut c_void)
}

unsafe extern "system" fn ShellExt_Release(this: *mut c_void) -> ULONG {
    let offset = std::mem::offset_of!(FileConverterShell, shell_ext_vtbl);
    let obj = (this as usize - offset) as *mut FileConverterShell;
    FileConverterShell_Release(obj as *mut c_void)
}

unsafe extern "system" fn FileConverterShell_QueryInterface_ContextMenu(
    this: *mut c_void,
    riid: *const GUID,
    ppv: *mut *mut c_void,
) -> HRESULT {
    let this_ptr = this as *mut FileConverterShell;
    FileConverterShell_QueryInterface(this_ptr, riid, ppv)
}

unsafe extern "system" fn FileConverterShell_QueryInterface_ShellExt(
    this: *mut c_void,
    riid: *const GUID,
    ppv: *mut *mut c_void,
) -> HRESULT {
    let offset = std::mem::offset_of!(FileConverterShell, shell_ext_vtbl);
    let this_ptr = (this as usize - offset) as *mut FileConverterShell;
    FileConverterShell_QueryInterface(this_ptr, riid, ppv)
}

unsafe fn FileConverterShell_QueryInterface(
    this: *mut FileConverterShell,
    riid: *const GUID,
    ppv: *mut *mut c_void,
) -> HRESULT {
    if ppv.is_null() {
        return E_FAIL;
    }
    *ppv = std::ptr::null_mut();

    if *riid == GUID::IID_IUNKNOWN || *riid == GUID::IID_ICONTEXTMENU {
        *ppv = this as *mut c_void;
    } else if *riid == GUID::IID_ISHELLEXTINIT {
        *ppv = &mut (*this).shell_ext_vtbl as *mut _ as *mut c_void;
    } else {
        return E_NOINTERFACE;
    }

    FileConverterShell_AddRef(this as *mut c_void);
    S_OK
}

unsafe extern "system" fn FileConverterShell_AddRef(this: *mut c_void) -> ULONG {
    let this = this as *mut FileConverterShell;
    let count = (*this).ref_count.fetch_add(1, Ordering::Relaxed) + 1;
    count
}

unsafe extern "system" fn FileConverterShell_Release(this: *mut c_void) -> ULONG {
    let this = this as *mut FileConverterShell;
    let count = (*this).ref_count.fetch_sub(1, Ordering::AcqRel) - 1;
    if count == 0 {
        std::ptr::drop_in_place(&mut (*this).selected_files);
        std::ptr::drop_in_place(&mut (*this).active_presets);
        let layout = std::alloc::Layout::new::<FileConverterShell>();
        std::alloc::dealloc(this as *mut u8, layout);
        G_OBJECT_COUNT.fetch_sub(1, Ordering::Relaxed);
    }
    count
}

unsafe extern "system" fn FileConverterShell_Initialize(
    this: *mut c_void,
    _pidlFolder: *const c_void,
    pDataObj: *mut IDataObject,
    _hkeyProgID: *mut c_void,
) -> HRESULT {
    let offset = std::mem::offset_of!(FileConverterShell, shell_ext_vtbl);
    let this_ptr = (this as usize - offset) as *mut FileConverterShell;

    if pDataObj.is_null() {
        return E_FAIL;
    }

    let fmt = FORMATETC {
        cfFormat: CF_HDROP,
        ptd: std::ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT,
        lindex: -1,
        tymed: TYMED_HGLOBAL,
    };

    let mut medium = STGMEDIUM {
        tymed: 0,
        hGlobal: std::ptr::null_mut(),
        pUnkForRelease: std::ptr::null_mut(),
    };

    let hr = ((*(*pDataObj).lpVtbl).GetData)(pDataObj, &fmt, &mut medium);
    if hr >= 0 {
        let hDrop = medium.hGlobal;
        let lock = GlobalLock(hDrop);
        if !lock.is_null() {
            let file_count = DragQueryFileW(hDrop, 0xFFFFFFFF, std::ptr::null_mut(), 0);
            let mut files = Vec::new();

            for i in 0..file_count {
                let size = DragQueryFileW(hDrop, i, std::ptr::null_mut(), 0);
                if size > 0 {
                    let mut buf = vec![0u16; (size + 1) as usize];
                    DragQueryFileW(hDrop, i, buf.as_mut_ptr(), size + 1);
                    if let Some(null_pos) = buf.iter().position(|&x| x == 0) {
                        let os_str = OsString::from_wide(&buf[..null_pos]);
                        if let Ok(path_str) = os_str.into_string() {
                            files.push(path_str);
                        }
                    }
                }
            }

            GlobalUnlock(hDrop);
            (*this_ptr).selected_files = files;
        }
        ReleaseStgMedium(&mut medium);
        S_OK
    } else {
        hr
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

fn get_extension_category(ext: &str) -> &'static str {
    match ext {
        "aac" | "aiff" | "ape" | "cda" | "flac" | "mp3" | "m4a" | "m4b" | "oga" | "ogg"
        | "opus" | "wav" | "wma" => "Audio",
        "3gp" | "3gpp" | "avi" | "bik" | "flv" | "m4v" | "mp4" | "mpg" | "mpeg" | "mov" | "mkv"
        | "ogv" | "rm" | "ts" | "vob" | "webm" | "wmv" => "Video",
        "arw" | "avif" | "bmp" | "cr2" | "dds" | "dng" | "exr" | "heic" | "ico" | "jfif"
        | "jpg" | "jpeg" | "nef" | "png" | "psd" | "raf" | "tga" | "tif" | "tiff" | "svg"
        | "xcf" | "webp" => "Image",
        "gif" => "Animated Image",
        "pdf" | "doc" | "docx" | "ppt" | "pptx" | "odp" | "ods" | "odt" | "xls" | "xlsx" => {
            "Document"
        }
        _ => "Misc",
    }
}

fn is_compatible(output_type: OutputType, category: &str) -> bool {
    if category == "Misc" {
        return true;
    }
    match output_type {
        OutputType::Aac
        | OutputType::Flac
        | OutputType::Mp3
        | OutputType::Ogg
        | OutputType::Wav => category == "Audio" || category == "Video",
        OutputType::Avi
        | OutputType::Mkv
        | OutputType::Mp4
        | OutputType::Ogv
        | OutputType::Webm => category == "Video" || category == "Animated Image",
        OutputType::Avif
        | OutputType::Ico
        | OutputType::Jpg
        | OutputType::Png
        | OutputType::Webp => {
            category == "Image" || category == "Document" || category == "Animated Image"
        }
        OutputType::Gif => {
            category == "Image" || category == "Video" || category == "Animated Image"
        }
        OutputType::Pdf => category == "Image" || category == "Document",
        OutputType::None => false,
    }
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
    is_compatible(preset.output_type, cat)
}

unsafe extern "system" fn FileConverterShell_QueryContextMenu(
    this: *mut c_void,
    hmenu: HMENU,
    indexMenu: u32,
    idCmdFirst: u32,
    _idCmdLast: u32,
    _uFlags: u32,
) -> HRESULT {
    let this = this as *mut FileConverterShell;
    if (*this).selected_files.is_empty() {
        return S_FALSE;
    }

    let mut settings = get_cached_settings();
    settings.merge(create_default_settings());

    let compatible_presets: Vec<_> = settings
        .conversion_presets
        .into_iter()
        .filter(|preset| {
            (*this)
                .selected_files
                .iter()
                .all(|file| is_preset_compatible_with_file(preset, file))
        })
        .collect();

    if compatible_presets.is_empty() {
        return S_FALSE;
    }

    // Store preset names on this instance so InvokeCommand always sees the list
    // that was used when building the menu, even if another COM object is alive
    // concurrently.
    (*this).active_presets.clear();
    for p in &compatible_presets {
        (*this).active_presets.push(p.name.clone());
    }
    (*this).configure_cmd_offset = None;

    let presets_count = compatible_presets.len();
    let cmd_id = idCmdFirst;
    let configure_cmd_id = cmd_id + presets_count as u32;

    let parent_text = "File Converter\0";
    let parent_text_wide: Vec<u16> = parent_text.encode_utf16().collect();

    if presets_count <= 5 {
        // Flat items directly on the context menu – no "Configure..." here.
        for (i, preset) in compatible_presets.iter().enumerate() {
            let mut name_wide: Vec<u16> = preset.name.encode_utf16().collect();
            name_wide.push(0);

            let mii = MENUITEMINFOW {
                cbSize: std::mem::size_of::<MENUITEMINFOW>() as u32,
                fMask: MIIM_STRING | MIIM_ID | MIIM_FTYPE,
                fType: MFT_STRING,
                fState: 0,
                wID: cmd_id + i as u32,
                hSubMenu: std::ptr::null_mut(),
                hbmpChecked: std::ptr::null_mut(),
                hbmpUnchecked: std::ptr::null_mut(),
                dwItemData: 0,
                dwTypeData: name_wide.as_ptr() as *mut u16,
                cch: (name_wide.len() - 1) as u32,
                hbmpItem: std::ptr::null_mut(),
            };

            InsertMenuItemW(hmenu, indexMenu + i as u32, 1, &mii);
        }

        (presets_count as i32).into()
    } else {
        // Cascading submenu with all presets + separator + "Configure..."
        let h_sub_menu = CreatePopupMenu();
        if h_sub_menu.is_null() {
            return E_FAIL;
        }

        for (i, preset) in compatible_presets.iter().enumerate() {
            let mut name_wide: Vec<u16> = preset.name.encode_utf16().collect();
            name_wide.push(0);

            let mii = MENUITEMINFOW {
                cbSize: std::mem::size_of::<MENUITEMINFOW>() as u32,
                fMask: MIIM_STRING | MIIM_ID | MIIM_FTYPE,
                fType: MFT_STRING,
                fState: 0,
                wID: cmd_id + i as u32,
                hSubMenu: std::ptr::null_mut(),
                hbmpChecked: std::ptr::null_mut(),
                hbmpUnchecked: std::ptr::null_mut(),
                dwItemData: 0,
                dwTypeData: name_wide.as_ptr() as *mut u16,
                cch: (name_wide.len() - 1) as u32,
                hbmpItem: std::ptr::null_mut(),
            };

            InsertMenuItemW(h_sub_menu, i as u32, 1, &mii);
        }

        let sep_mii = MENUITEMINFOW {
            cbSize: std::mem::size_of::<MENUITEMINFOW>() as u32,
            fMask: MIIM_FTYPE,
            fType: MFT_SEPARATOR,
            fState: 0,
            wID: 0,
            hSubMenu: std::ptr::null_mut(),
            hbmpChecked: std::ptr::null_mut(),
            hbmpUnchecked: std::ptr::null_mut(),
            dwItemData: 0,
            dwTypeData: std::ptr::null_mut(),
            cch: 0,
            hbmpItem: std::ptr::null_mut(),
        };
        InsertMenuItemW(h_sub_menu, presets_count as u32, 1, &sep_mii);

        let config_text = "Configure...\0";
        let mut config_text_wide: Vec<u16> = config_text.encode_utf16().collect();
        let config_mii = MENUITEMINFOW {
            cbSize: std::mem::size_of::<MENUITEMINFOW>() as u32,
            fMask: MIIM_STRING | MIIM_ID | MIIM_FTYPE,
            fType: MFT_STRING,
            fState: 0,
            wID: configure_cmd_id,
            hSubMenu: std::ptr::null_mut(),
            hbmpChecked: std::ptr::null_mut(),
            hbmpUnchecked: std::ptr::null_mut(),
            dwItemData: 0,
            dwTypeData: config_text_wide.as_mut_ptr(),
            cch: (config_text_wide.len() - 1) as u32,
            hbmpItem: std::ptr::null_mut(),
        };
        InsertMenuItemW(h_sub_menu, (presets_count + 1) as u32, 1, &config_mii);

        // Record which verb offset corresponds to "Configure..."
        (*this).configure_cmd_offset = Some(presets_count);

        let parent_mii = MENUITEMINFOW {
            cbSize: std::mem::size_of::<MENUITEMINFOW>() as u32,
            fMask: MIIM_STRING | MIIM_SUBMENU | MIIM_FTYPE,
            fType: MFT_STRING,
            fState: 0,
            wID: 0,
            hSubMenu: h_sub_menu,
            hbmpChecked: std::ptr::null_mut(),
            hbmpUnchecked: std::ptr::null_mut(),
            dwItemData: 0,
            dwTypeData: parent_text_wide.as_ptr() as *mut u16,
            cch: (parent_text_wide.len() - 1) as u32,
            hbmpItem: std::ptr::null_mut(),
        };

        InsertMenuItemW(hmenu, indexMenu, 1, &parent_mii);

        // IDs used: preset[0..N] + configure = N+1 commands total
        // (separator has no ID so doesn't count)
        (presets_count as i32 + 1).into()
    }
}

unsafe extern "system" fn FileConverterShell_InvokeCommand(
    this: *mut c_void,
    pici: *const CMINVOKECOMMANDINFO,
) -> HRESULT {
    let this = this as *mut FileConverterShell;
    if pici.is_null() {
        return E_FAIL;
    }

    // lpVerb is a MAKEINTRESOURCE value when its high word is 0 – the low word
    // is the zero-based offset from idCmdFirst that was returned by QueryContextMenu.
    let verb_val = (*pici).lpVerb as usize;
    if verb_val >> 16 != 0 {
        // String verb – not handled
        return E_FAIL;
    }
    let verb_offset = verb_val & 0xFFFF;

    let mut presets = (*this).active_presets.clone();
    if presets.is_empty() {
        let mut settings = get_cached_settings();
        settings.merge(create_default_settings());

        let categories: Vec<String> = (*this)
            .selected_files
            .iter()
            .map(|f| {
                let ext = Path::new(f)
                    .extension()
                    .and_then(|s| s.to_str())
                    .unwrap_or("");
                get_extension_category(&ext.to_lowercase()).to_string()
            })
            .collect();

        presets = settings
            .conversion_presets
            .into_iter()
            .filter(|preset| {
                (*this)
                    .selected_files
                    .iter()
                    .all(|file| is_preset_compatible_with_file(preset, file))
            })
            .map(|p| p.name)
            .collect();
    }

    let presets_count = presets.len();

    if verb_offset < presets_count {
        // A conversion preset was chosen.
        let preset_name = &presets[verb_offset];

        let bin_path = get_bin_path();
        if !bin_path.exists() {
            return E_FAIL;
        }

        let mut cmd = Command::new(&bin_path);
        cmd.arg("--conversion-preset").arg(preset_name);

        let mut total_len = preset_name.len() + 30;
        for file in &(*this).selected_files {
            total_len += file.len() + 3;
        }

        if total_len >= 8000 {
            let temp_dir = std::env::temp_dir();
            let pid = std::process::id();
            let temp_file_path = temp_dir.join(format!("file-converter-input-list-{}.txt", pid));

            if let Ok(mut file) = std::fs::File::create(&temp_file_path) {
                use std::io::Write;
                for path in &(*this).selected_files {
                    let _ = writeln!(file, "{}", path);
                }
                cmd.arg("--input-files").arg(&temp_file_path);
            } else {
                for file in &(*this).selected_files {
                    cmd.arg(file);
                }
            }
        } else {
            for file in &(*this).selected_files {
                cmd.arg(file);
            }
        }

        if cmd.spawn().is_ok() {
            S_OK
        } else {
            E_FAIL
        }
    } else {
        // "Configure..." item from the submenu was chosen.
        let bin_path = get_bin_path();
        if bin_path.exists() {
            let _ = Command::new(&bin_path).arg("-settings").spawn();
            S_OK
        } else {
            E_FAIL
        }
    }
}

unsafe extern "system" fn FileConverterShell_GetCommandString(
    _this: *mut c_void,
    _idCmd: usize,
    _uFlags: u32,
    _pwzReserved: *mut u32,
    _pszName: *mut u8,
    _cchMax: u32,
) -> HRESULT {
    S_OK
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
    if let Ok(mut exe_path) = std::env::current_exe() {
        exe_path.pop();
        let path = exe_path.join("file_converter_bin.exe");
        if path.exists() {
            return path;
        }
    }
    let dll_hinst = G_DLL_INSTANCE.load(Ordering::Relaxed) as *mut c_void;
    if !dll_hinst.is_null() {
        let mut buf = vec![0u16; 512];
        let len = unsafe { GetModuleFileNameW(dll_hinst, buf.as_mut_ptr(), 512) };
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

    PathBuf::from("file_converter_bin.exe")
}

// Class Factory implementation for COM registration
#[repr(C)]
struct FileConverterClassFactory {
    vtbl: *const IClassFactoryVtbl,
    ref_count: AtomicU32,
}

static CLASS_FACTORY_VTBL: IClassFactoryVtbl = IClassFactoryVtbl {
    QueryInterface: ClassFactory_QueryInterface,
    AddRef: ClassFactory_AddRef,
    Release: ClassFactory_Release,
    CreateInstance: ClassFactory_CreateInstance,
    LockServer: ClassFactory_LockServer,
};

unsafe extern "system" fn ClassFactory_QueryInterface(
    this: *mut c_void,
    riid: *const GUID,
    ppv: *mut *mut c_void,
) -> HRESULT {
    if ppv.is_null() {
        return E_FAIL;
    }
    *ppv = std::ptr::null_mut();

    if *riid == GUID::IID_IUNKNOWN || *riid == GUID::IID_ICLASSFACTORY {
        *ppv = this;
        ClassFactory_AddRef(this);
        S_OK
    } else {
        E_NOINTERFACE
    }
}

unsafe extern "system" fn ClassFactory_AddRef(this: *mut c_void) -> ULONG {
    let this = this as *mut FileConverterClassFactory;
    (*this).ref_count.fetch_add(1, Ordering::Relaxed) + 1
}

unsafe extern "system" fn ClassFactory_Release(this: *mut c_void) -> ULONG {
    let this = this as *mut FileConverterClassFactory;
    let count = (*this).ref_count.fetch_sub(1, Ordering::AcqRel) - 1;
    if count == 0 {
        let layout = std::alloc::Layout::new::<FileConverterClassFactory>();
        std::alloc::dealloc(this as *mut u8, layout);
    }
    count
}

unsafe extern "system" fn ClassFactory_CreateInstance(
    _this: *mut c_void,
    pUnkOuter: *mut c_void,
    riid: *const GUID,
    ppvObject: *mut *mut c_void,
) -> HRESULT {
    if !pUnkOuter.is_null() {
        return -2147221232; // CLASS_E_NOAGGREGATION
    }

    let layout = std::alloc::Layout::new::<FileConverterShell>();
    let obj_ptr = std::alloc::alloc(layout) as *mut FileConverterShell;
    if obj_ptr.is_null() {
        return E_OUTOFMEMORY;
    }

    std::ptr::write(
        obj_ptr,
        FileConverterShell {
            context_menu_vtbl: &CONTEXT_MENU_VTBL,
            shell_ext_vtbl: &SHELL_EXT_VTBL,
            ref_count: AtomicU32::new(1),
            selected_files: Vec::new(),
            active_presets: Vec::new(),
            configure_cmd_offset: None,
        },
    );

    G_OBJECT_COUNT.fetch_add(1, Ordering::Relaxed);

    let hr = FileConverterShell_QueryInterface(obj_ptr, riid, ppvObject);
    FileConverterShell_Release(obj_ptr as *mut c_void);
    hr
}

unsafe extern "system" fn ClassFactory_LockServer(_this: *mut c_void, fLock: i32) -> HRESULT {
    if fLock != 0 {
        G_LOCK_COUNT.fetch_add(1, Ordering::Relaxed);
    } else {
        G_LOCK_COUNT.fetch_sub(1, Ordering::Relaxed);
    }
    S_OK
}

// COM DLL Export Server entry points
#[no_mangle]
pub unsafe extern "system" fn DllGetClassObject(
    rclsid: *const GUID,
    riid: *const GUID,
    ppv: *mut *mut c_void,
) -> HRESULT {
    if ppv.is_null() {
        return E_FAIL;
    }
    *ppv = std::ptr::null_mut();

    if *rclsid != GUID::CLSID_FILE_CONVERTER {
        return -2147221231; // CLASS_E_CLASSNOTAVAILABLE
    }

    let layout = std::alloc::Layout::new::<FileConverterClassFactory>();
    let factory_ptr = std::alloc::alloc(layout) as *mut FileConverterClassFactory;
    if factory_ptr.is_null() {
        return E_OUTOFMEMORY;
    }

    std::ptr::write(
        factory_ptr,
        FileConverterClassFactory {
            vtbl: &CLASS_FACTORY_VTBL,
            ref_count: AtomicU32::new(1),
        },
    );

    let hr = ClassFactory_QueryInterface(factory_ptr as *mut c_void, riid, ppv);
    ClassFactory_Release(factory_ptr as *mut c_void);
    hr
}

#[no_mangle]
pub unsafe extern "system" fn DllCanUnloadNow() -> HRESULT {
    if G_LOCK_COUNT.load(Ordering::Relaxed) == 0 && G_OBJECT_COUNT.load(Ordering::Relaxed) == 0 {
        S_OK
    } else {
        S_FALSE
    }
}

// Regsvr32 entries
#[cfg(target_os = "windows")]
#[no_mangle]
pub unsafe extern "system" fn DllRegisterServer() -> HRESULT {
    use winreg::enums::{HKEY_CLASSES_ROOT, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_ALL_ACCESS};
    use winreg::RegKey;

    let hmodule = G_DLL_INSTANCE.load(Ordering::Relaxed) as *mut c_void;
    let mut module_path = PathBuf::new();

    if !hmodule.is_null() {
        let mut buf = vec![0u16; 512];
        let len = GetModuleFileNameW(hmodule, buf.as_mut_ptr(), 512);
        if len > 0 {
            let os_str = OsString::from_wide(&buf[..len as usize]);
            module_path = PathBuf::from(os_str);
        }
    }

    if module_path.as_os_str().is_empty() {
        let dll_name: Vec<u16> = "file_converter_shell.dll\0".encode_utf16().collect();
        let h = GetModuleHandleW(dll_name.as_ptr());
        if !h.is_null() {
            let mut buf = vec![0u16; 512];
            let len = GetModuleFileNameW(h, buf.as_mut_ptr(), 512);
            if len > 0 {
                let os_str = OsString::from_wide(&buf[..len as usize]);
                module_path = PathBuf::from(os_str);
            }
        }
    }

    let clsid_str = "{AF9B72B5-F4E4-44B0-A3D9-B55B748EFE90}";
    let hkcr = RegKey::predef(HKEY_CLASSES_ROOT);
    let hklm_classes = RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey_with_flags("Software\\Classes", KEY_ALL_ACCESS);
    let hkcu_classes = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey_with_flags("Software\\Classes", KEY_ALL_ACCESS);

    let clsid_path = format!("CLSID\\{}", clsid_str);
    let clsid_inproc_path = format!("CLSID\\{}\\InprocServer32", clsid_str);

    let _ = hkcr.create_subkey(&clsid_path);
    if let Ok((key, _)) = hkcr.create_subkey(&clsid_inproc_path) {
        let _ = key.set_value("", &module_path.to_string_lossy().to_string());
        let _ = key.set_value("ThreadingModel", &"Apartment");
    }

    if let Ok(ref root) = hklm_classes {
        let _ = root.create_subkey(&clsid_path);
        if let Ok((key, _)) = root.create_subkey(&clsid_inproc_path) {
            let _ = key.set_value("", &module_path.to_string_lossy().to_string());
            let _ = key.set_value("ThreadingModel", &"Apartment");
        }
    }
    if let Ok(ref root) = hkcu_classes {
        let _ = root.create_subkey(&clsid_path);
        if let Ok((key, _)) = root.create_subkey(&clsid_inproc_path) {
            let _ = key.set_value("", &module_path.to_string_lossy().to_string());
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
        if let Ok((key, _)) = hkcr.create_subkey(&path) {
            let _ = key.set_value("", &clsid_str);
        }
        if let Ok(ref root) = hklm_classes {
            if let Ok((key, _)) = root.create_subkey(&path) {
                let _ = key.set_value("", &clsid_str);
            }
        }
        if let Ok(ref root) = hkcu_classes {
            if let Ok((key, _)) = root.create_subkey(&path) {
                let _ = key.set_value("", &clsid_str);
            }
        }

        let verb_path = format!("{}\\shell\\FileConverter", assoc);
        let verb_cmd_path = format!("{}\\shell\\FileConverter\\command", assoc);
        let bin_path = get_bin_path();
        if let Ok((key, _)) = hkcr.create_subkey(&verb_path) {
            let _ = key.set_value("MUIVerb", &"File Converter");
            let _ = key.set_value("SubCommands", &"");
        }
        if let Ok((key, _)) = hkcr.create_subkey(&verb_cmd_path) {
            let cmd_str = format!("\"{}\" -settings", bin_path.to_string_lossy());
            let _ = key.set_value("", &cmd_str);
        }
    }

    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    if let Ok((key, _)) = hklm
        .create_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Shell Extensions\\Approved")
    {
        let _ = key.set_value(clsid_str, &"File Converter Context Menu Handler");
    }

    #[link(name = "shell32")]
    unsafe extern "system" {
        fn SHChangeNotify(
            wEventId: i32,
            uFlags: u32,
            dwItem1: *const c_void,
            dwItem2: *const c_void,
        );
    }
    SHChangeNotify(
        0x08000000, /* SHCNE_ASSOCCHANGED */
        0x0000,     /* SHCNF_IDLIST */
        std::ptr::null(),
        std::ptr::null(),
    );

    S_OK
}

#[cfg(target_os = "windows")]
#[no_mangle]
pub unsafe extern "system" fn DllUnregisterServer() -> HRESULT {
    use winreg::enums::{HKEY_CLASSES_ROOT, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_ALL_ACCESS};
    use winreg::RegKey;

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

    #[link(name = "shell32")]
    unsafe extern "system" {
        fn SHChangeNotify(
            wEventId: i32,
            uFlags: u32,
            dwItem1: *const c_void,
            dwItem2: *const c_void,
        );
    }
    SHChangeNotify(
        0x08000000, /* SHCNE_ASSOCCHANGED */
        0x0000,     /* SHCNF_IDLIST */
        std::ptr::null(),
        std::ptr::null(),
    );

    S_OK
}
