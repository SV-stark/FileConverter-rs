//! Windows Explorer context-menu shell extension (in-process COM DLL).
//!
//! Right-clicking files in Explorer shows a "File Converter" sub-menu listing
//! the conversion presets that fit the selection, exactly like the C#
//! original: presets filed in folders appear as nested sub-menus, presets that
//! only fit part of a mixed selection are shown greyed out, and the list ends
//! with "Configure presets...". Choosing a preset starts the conversion
//! straight away; the settings window is never opened for it.
//!
//! The menu is a classic `IContextMenu` handler registered under
//! `HKCR\*\shellex\ContextMenuHandlers`. On Windows 11 it is in the full menu
//! ("Show more options", or Shift+right-click), the same place as the C# app.

#![allow(
    // The `windows` crate's generated COM plumbing forces snake_case / non-CamelCase
    // identifiers that we cannot rename; this blanket applies only to this FFI crate.
    non_snake_case,
    non_camel_case_types,
    reason = "Win32 COM FFI surface is generated names we cannot rename"
)]

use std::ffi::{OsString, c_void};
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, OnceLock, RwLock};
use std::time::SystemTime;

use file_converter_core::settings::{ConversionPreset, Settings};
use file_converter_core::types::{OutputType, is_preset_applicable_to_file};

#[allow(
    unused_imports,
    reason = "re-exported for the shell extension's COM interop"
)]
mod windows_core {
    pub use windows::core::*;
}
use windows::Win32::Foundation::{
    BOOL, CLASS_E_CLASSNOTAVAILABLE, CLASS_E_NOAGGREGATION, E_FAIL, E_NOTIMPL, HANDLE, HMODULE,
    LPARAM, S_FALSE, S_OK,
};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS,
    DeleteDC, DeleteObject, HBITMAP, HDC, HGDIOBJ, SelectObject,
};
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
    CMINVOKECOMMANDINFO, DragQueryFileW, ECF_DEFAULT, ECS_ENABLED, ExtractIconExW, HDROP,
    IContextMenu, IContextMenu_Impl, IEnumExplorerCommand, IExplorerCommand, IExplorerCommand_Impl,
    IShellExtInit, IShellExtInit_Impl, IShellItemArray, IShellPropSheetExt,
    IShellPropSheetExt_Impl, SHCNE_ASSOCCHANGED, SHCNF_IDLIST, SHChangeNotify, SHGSI_ICON,
    SHGSI_SMALLICON, SHGetStockIconInfo, SHSTOCKICONID, SHSTOCKICONINFO, SIGDN_FILESYSPATH,
};

type LPFNADDPROPSHEETPAGE = Option<unsafe extern "system" fn(HPROPSHEETPAGE, LPARAM) -> BOOL>;
use windows::Win32::UI::WindowsAndMessaging::{
    CreatePopupMenu, DI_NORMAL, DestroyIcon, DestroyMenu, DrawIconEx, GetSystemMetrics, HICON,
    HMENU, InsertMenuItemW, MENUITEMINFOW, MFS_DISABLED, MFT_SEPARATOR, MFT_STRING, MIIM_BITMAP,
    MIIM_FTYPE, MIIM_ID, MIIM_STATE, MIIM_STRING, MIIM_SUBMENU, SM_CXSMICON,
};

const CLSID_FILE_CONVERTER: GUID = GUID::from_u128(0xAF9B72B5_F4E4_44B0_A3D9_B55B748EFE90);
const CLSID_STR: &str = "{AF9B72B5-F4E4-44B0-A3D9-B55B748EFE90}";

/// Every class the extension (or an older version of it, or the C#
/// original) has ever been registered under. Registration cleans all of them
/// so leftovers can't show a second "File Converter" entry.
const ALL_ASSOCIATIONS: [&str; 6] = [
    "*",
    "AllFilesystemObjects",
    "Directory",
    "Directory\\Background",
    "Drive",
    "Folder",
];

// Atomic DLL instance handle & active COM object counters
static G_DLL_INSTANCE: AtomicUsize = AtomicUsize::new(0);
static G_LOCK_COUNT: AtomicU32 = AtomicU32::new(0);
static G_OBJECT_COUNT: AtomicU32 = AtomicU32::new(0);

/// # Safety
///
/// COM entry point invoked by the Windows loader under the loader lock when
/// this DLL is attached to `explorer.exe`. `hinst_dll` must be the module
/// handle the loader supplied; the function performs no allocation, formatting,
/// or locking, so the loader-lock invariant is preserved.
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
        // SAFETY: `hinst_dll` is the loader-supplied module handle for this DLL.
        unsafe {
            let _ = DisableThreadLibraryCalls(hinst_dll);
        }
    } else if fdw_reason == 0 && _lpv_reserved.is_null() {
        // DLL_PROCESS_DETACH because Explorer unloaded us (not process exit):
        // give back the cached menu bitmaps so each unload/reload cycle does
        // not leave eight GDI handles behind. `OnceLock::get` neither locks
        // nor allocates.
        if let Some(icons) = MENU_BITMAPS.get() {
            for raw in [
                icons.app,
                icons.folder,
                icons.settings,
                icons.audio,
                icons.video,
                icons.image,
                icons.pdf,
                icons.document,
            ] {
                if raw != 0 {
                    // SAFETY: the bitmaps are ours and no menu can still be
                    // showing them once the DLL is being unloaded.
                    unsafe {
                        let _ = DeleteObject(HBITMAP(raw as *mut c_void));
                    }
                }
            }
        }
    }
    1
}

/// The single canonical copy of the default presets, embedded at build time.
///
/// It lives at the repository root (shared with `file_converter_bin`, the
/// installer and the release packaging). Do **not** add a crate-local copy: the
/// duplicates that used to exist here silently shipped stale presets.
const DEFAULT_SETTINGS_XML: &str = include_str!("../../Settings.default.xml");

// ---------------------------------------------------------------------------
// Menu icons
// ---------------------------------------------------------------------------

/// Menu bitmaps, created once per Explorer process and kept for its lifetime.
///
/// `MIIM_BITMAP` does **not** hand the bitmap to the menu (`DestroyMenu` never
/// deletes it), so creating fresh bitmaps on every right-click leaked GDI
/// handles inside explorer.exe until it hit the 10,000 limit and menus and
/// icons stopped drawing. Stored as raw handle values because `HBITMAP` is not
/// `Send`/`Sync`; 0 means "no icon".
struct MenuBitmaps {
    app: usize,
    folder: usize,
    settings: usize,
    audio: usize,
    video: usize,
    image: usize,
    pdf: usize,
    document: usize,
}

static MENU_BITMAPS: OnceLock<MenuBitmaps> = OnceLock::new();

fn menu_bitmaps() -> &'static MenuBitmaps {
    MENU_BITMAPS.get_or_init(|| {
        // SAFETY: GetSystemMetrics has no preconditions.
        let size = unsafe { GetSystemMetrics(SM_CXSMICON) }.clamp(12, 64);
        let app = app_icon(size).map_or(0, |b| b.0 as usize);
        MenuBitmaps {
            app,
            // SIID_FOLDER = 3. The stock "settings" icon draws blank in
            // menus on Windows 11, so the gear is drawn here instead.
            folder: stock_icon(SHSTOCKICONID(3), size).map_or(0, |b| b.0 as usize),
            settings: gear_bitmap(size).map_or(0, |b| b.0 as usize),
            audio: badge_bitmap(rgb(232, 128, 0), size).map_or(0, |b| b.0 as usize),
            video: badge_bitmap(rgb(40, 104, 224), size).map_or(0, |b| b.0 as usize),
            image: badge_bitmap(rgb(40, 160, 64), size).map_or(0, |b| b.0 as usize),
            pdf: badge_bitmap(rgb(208, 40, 40), size).map_or(0, |b| b.0 as usize),
            document: badge_bitmap(rgb(112, 112, 128), size).map_or(0, |b| b.0 as usize),
        }
    })
}

fn category_bitmap(output_type: OutputType) -> HBITMAP {
    let icons = menu_bitmaps();
    let raw = match output_type {
        OutputType::Pdf => icons.pdf,
        other => match other.kind() {
            "Audio" => icons.audio,
            "Video" => icons.video,
            "Image" => icons.image,
            _ => icons.document,
        },
    };
    HBITMAP(raw as *mut c_void)
}

const fn rgb(r: u8, g: u8, b: u8) -> [u8; 3] {
    [r, g, b]
}

/// Creates an empty top-down 32-bit premultiplied-alpha DIB section, the
/// format menus alpha-blend, and returns it with a pointer to its pixels.
fn create_argb_bitmap(size: i32) -> Option<(HBITMAP, *mut u32)> {
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: size,
            biHeight: -size, // negative = top-down rows
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits: *mut c_void = std::ptr::null_mut();
    // SAFETY: `info` is a valid BITMAPINFO for the duration of the call and
    // `bits` receives the pixel pointer owned by the returned section.
    let bmp = unsafe {
        CreateDIBSection(
            HDC::default(),
            &info,
            DIB_RGB_COLORS,
            &mut bits,
            HANDLE::default(),
            0,
        )
    }
    .ok()?;
    if bits.is_null() {
        // SAFETY: `bmp` was just created and is not selected anywhere.
        unsafe {
            let _ = DeleteObject(bmp);
        }
        return None;
    }
    Some((bmp, bits as *mut u32))
}

/// A small rounded square in the category colour, anti-aliased.
fn badge_bitmap(color: [u8; 3], size: i32) -> Option<HBITMAP> {
    let (bmp, bits) = create_argb_bitmap(size)?;
    let s = size as f32;
    let inset = (s * 0.12).round();
    let (lo, hi) = (inset, s - inset);
    let radius = ((hi - lo) * 0.28).max(2.0);
    // SAFETY: the DIB section holds exactly `size * size` 32-bit pixels.
    let pixels = unsafe { std::slice::from_raw_parts_mut(bits, (size * size) as usize) };
    for y in 0..size {
        for x in 0..size {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            // Signed distance to the rounded rectangle.
            let cx = px.clamp(lo + radius, hi - radius);
            let cy = py.clamp(lo + radius, hi - radius);
            let d = ((px - cx).powi(2) + (py - cy).powi(2)).sqrt() - radius;
            let coverage = (0.5 - d).clamp(0.0, 1.0);
            // A slightly lighter top half gives the badge some depth.
            let lift = if py < s / 2.0 { 18.0 } else { 0.0 };
            let ch = |c: u8| ((c as f32 + lift).min(255.0) * coverage).round() as u32;
            let a = (coverage * 255.0).round() as u32;
            pixels[(y * size + x) as usize] =
                (a << 24) | (ch(color[0]) << 16) | (ch(color[1]) << 8) | ch(color[2]);
        }
    }
    Some(bmp)
}

/// A grey eight-tooth gear for "Configure presets...", anti-aliased. Mid grey
/// reads on both the light and the dark Explorer menu.
fn gear_bitmap(size: i32) -> Option<HBITMAP> {
    let (bmp, bits) = create_argb_bitmap(size)?;
    let s = size as f32;
    let c = s / 2.0;
    let body = s * 0.30;
    let tooth = s * 0.12;
    let hole = s * 0.13;
    let [r, g, b] = rgb(110, 110, 120);
    // SAFETY: the DIB section holds exactly `size * size` 32-bit pixels.
    let pixels = unsafe { std::slice::from_raw_parts_mut(bits, (size * size) as usize) };
    for y in 0..size {
        for x in 0..size {
            let (dx, dy) = (x as f32 + 0.5 - c, y as f32 + 0.5 - c);
            let dist = (dx * dx + dy * dy).sqrt();
            let angle = dy.atan2(dx);
            // Teeth: a smoothed square wave around the rim.
            let wave = ((angle * 8.0).cos() * 3.0).clamp(-1.0, 1.0) * 0.5 + 0.5;
            let outer = body + tooth * wave;
            let coverage = (outer - dist + 0.5)
                .clamp(0.0, 1.0)
                .min((dist - hole + 0.5).clamp(0.0, 1.0));
            let a = (coverage * 255.0).round() as u32;
            let ch = |v: u8| (v as f32 * coverage).round() as u32;
            pixels[(y * size + x) as usize] = (a << 24) | (ch(r) << 16) | (ch(g) << 8) | ch(b);
        }
    }
    Some(bmp)
}

/// Draws an icon into a new premultiplied-alpha bitmap and destroys the icon.
fn icon_to_bitmap(icon: HICON, size: i32) -> Option<HBITMAP> {
    let _destroy = scopeguard::guard(icon, |i| {
        // SAFETY: the icon was created for us and is not used after this.
        unsafe {
            let _ = DestroyIcon(i);
        }
    });
    let (bmp, _bits) = create_argb_bitmap(size)?;
    // SAFETY: a memory DC is created, the fresh DIB section is selected into
    // it for the draw and deselected before the DC is deleted.
    unsafe {
        let dc = CreateCompatibleDC(HDC::default());
        if dc.is_invalid() {
            let _ = DeleteObject(bmp);
            return None;
        }
        let old = SelectObject(dc, HGDIOBJ(bmp.0));
        let drawn = DrawIconEx(
            dc,
            0,
            0,
            icon,
            size,
            size,
            0,
            windows::Win32::Graphics::Gdi::HBRUSH::default(),
            DI_NORMAL,
        );
        SelectObject(dc, old);
        let _ = DeleteDC(dc);
        if drawn.is_err() {
            let _ = DeleteObject(bmp);
            return None;
        }
    }
    Some(bmp)
}

fn stock_icon(id: SHSTOCKICONID, size: i32) -> Option<HBITMAP> {
    let mut info = SHSTOCKICONINFO {
        cbSize: std::mem::size_of::<SHSTOCKICONINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: `info` is a correctly sized, writable SHSTOCKICONINFO.
    unsafe { SHGetStockIconInfo(id, SHGSI_ICON | SHGSI_SMALLICON, &mut info) }.ok()?;
    if info.hIcon.is_invalid() {
        return None;
    }
    icon_to_bitmap(info.hIcon, size)
}

/// The File Converter icon, taken from the converter executable.
fn app_icon(size: i32) -> Option<HBITMAP> {
    let bin = get_bin_path();
    if !bin.exists() {
        return None;
    }
    let wide: Vec<u16> = bin.as_os_str().encode_wide_nul();
    let mut small = HICON::default();
    // SAFETY: `wide` is a NUL-terminated path that outlives the call and
    // `small` receives at most one icon handle, which we then own.
    let count = unsafe { ExtractIconExW(PCWSTR(wide.as_ptr()), 0, None, Some(&mut small), 1) };
    if count == 0 || small.is_invalid() {
        return None;
    }
    icon_to_bitmap(small, size)
}

/// `OsStr` -> NUL-terminated UTF-16.
trait EncodeWideNul {
    fn encode_wide_nul(&self) -> Vec<u16>;
}

impl EncodeWideNul for std::ffi::OsStr {
    fn encode_wide_nul(&self) -> Vec<u16> {
        use std::os::windows::ffi::OsStrExt;
        self.encode_wide().chain(std::iter::once(0)).collect()
    }
}

fn wide_nul(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

// ---------------------------------------------------------------------------
// Selection and menu contents
// ---------------------------------------------------------------------------

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
                    files.reserve(file_count as usize);
                    for i in 0..file_count {
                        let size = DragQueryFileW(HDROP(ptr), i, None);
                        if size == 0 {
                            continue;
                        }
                        let mut buf = vec![0u16; size as usize + 1];
                        let written = DragQueryFileW(HDROP(ptr), i, Some(&mut buf));
                        // The fill must actually happen: otherwise `buf` stays
                        // zeroed and yields an empty path.
                        if written == 0 {
                            continue;
                        }
                        let len = (written as usize).min(buf.len() - 1);
                        if let Ok(path_str) = OsString::from_wide(&buf[..len]).into_string() {
                            files.push(path_str);
                        }
                    }
                }
            }
        }
    }
    files
}

/// One preset in the File Converter sub-menu.
#[derive(Debug, Clone, PartialEq)]
struct MenuEntry {
    /// Full preset name, passed to the converter (`"Scale 75%/To Png"`).
    preset_name: String,
    /// Sub-menus it is filed under (`["Scale 75%"]`).
    folders: Vec<String>,
    /// Text shown in the menu (`"To Png"`).
    label: String,
    output_type: OutputType,
    /// False when the preset only fits some of the selected files: it is
    /// shown greyed out, as in the C# original.
    enabled: bool,
}

/// The lower-cased extensions of the selection, each listed once. Items
/// without an extension (folders, extension-less files) are ignored, as in
/// the C# original.
fn selection_extensions(files: &[String]) -> Vec<String> {
    let mut exts: Vec<String> = Vec::new();
    for file in files {
        let Some(ext) = Path::new(file).extension().and_then(|e| e.to_str()) else {
            continue;
        };
        if ext.is_empty() {
            continue;
        }
        let ext = ext.to_ascii_lowercase();
        if !exts.contains(&ext) {
            exts.push(ext);
        }
    }
    exts
}

/// What the File Converter sub-menu lists for a selection (C# parity).
///
/// A preset is listed when it fits at least one of the selected file types
/// and enabled only when it fits all of them. Each file type is checked once,
/// so selecting thousands of photos costs no more than selecting one.
fn plan_menu(presets: &[ConversionPreset], files: &[String]) -> Vec<MenuEntry> {
    let exts = selection_extensions(files);
    if exts.is_empty() {
        return Vec::new();
    }
    let samples: Vec<String> = exts.iter().map(|e| format!("x.{e}")).collect();

    let mut entries = Vec::new();
    for preset in presets {
        if preset.short_name().trim().is_empty() {
            continue;
        }
        let declared: Vec<String> = preset.input_types.iter().map(|s| s.to_string()).collect();
        let fitting = samples
            .iter()
            .filter(|sample| is_preset_applicable_to_file(preset.output_type, &declared, sample))
            .count();
        if fitting == 0 {
            continue;
        }
        entries.push(MenuEntry {
            preset_name: preset.name.to_string(),
            folders: preset
                .folders()
                .iter()
                .map(|f| f.trim().to_string())
                .collect(),
            label: preset.short_name().trim().to_string(),
            output_type: preset.output_type,
            enabled: fitting == samples.len(),
        });
    }
    entries
}

#[implement(IShellExtInit, IContextMenu, IShellPropSheetExt, IExplorerCommand)]
struct FileConverterShellExt {
    selected_files: RwLock<Vec<String>>,
    /// The entries shown by the last `QueryContextMenu`, in command-id order.
    menu_entries: RwLock<Vec<MenuEntry>>,
}

impl FileConverterShellExt {
    fn new() -> Self {
        G_OBJECT_COUNT.fetch_add(1, Ordering::Relaxed);
        Self {
            selected_files: RwLock::new(Vec::new()),
            menu_entries: RwLock::new(Vec::new()),
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

/// The embedded default presets, parsed once per Explorer process instead of
/// on every right-click (the XML is ~100 KB).
static DEFAULT_SETTINGS: LazyLock<Settings> = LazyLock::new(create_default_settings);

// In-memory thread-safe cache of the *merged* (user + default) settings, keyed
// on the user file's modification time. Right-clicks no longer re-read,
// re-parse and re-merge anything unless the file actually changed.
/// Merged settings plus the user file's modification time they were read at.
type SettingsCache = Option<(SystemTime, Arc<Settings>)>;

static CACHED_SETTINGS: LazyLock<RwLock<SettingsCache>> = LazyLock::new(|| RwLock::new(None));

fn get_cached_settings() -> Arc<Settings> {
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
                return Arc::clone(settings);
            }
        }
    }

    let mut loaded = if user_settings_path.exists() {
        Settings::load_from_file(&user_settings_path).unwrap_or_else(|_| DEFAULT_SETTINGS.clone())
    } else {
        DEFAULT_SETTINGS.clone()
    };
    loaded.merge(DEFAULT_SETTINGS.clone());
    let loaded = Arc::new(loaded);

    if let Ok(mut guard) = CACHED_SETTINGS.write() {
        *guard = Some((mtime, Arc::clone(&loaded)));
    }

    loaded
}

#[cfg(test)]
fn is_preset_compatible_with_file(preset: &ConversionPreset, file_path: &str) -> bool {
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
        let Ok(file) = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        else {
            continue;
        };
        let mut file = std::io::BufWriter::new(file);

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

/// Windows' command-line limit is 32,767 characters, but the C# original
/// switches to a list file at 8,000 and so do we.
const MAX_COMMAND_LINE: usize = 8000;

/// Starts the converter for `files` with `preset_name`.
fn launch_conversion(preset_name: &str, files: &[String]) -> Result<()> {
    // Folders can be part of a mixed selection; only files are converted.
    // Checked here, once, rather than on every right-click.
    let files: Vec<String> = files
        .iter()
        .filter(|f| Path::new(f).is_file())
        .cloned()
        .collect();
    if files.is_empty() {
        return Err(E_FAIL.into());
    }

    let bin_path = get_bin_path();
    if !bin_path.exists() {
        dbg_log!("Converter binary not found: {}", bin_path.display());
        return Err(E_FAIL.into());
    }

    let mut cmd = Command::new(&bin_path);
    cmd.arg("--conversion-preset").arg(preset_name);

    let total_len = preset_name.len() + 30 + files.iter().map(|f| f.len() + 3).sum::<usize>();
    let list = (total_len >= MAX_COMMAND_LINE)
        .then(|| write_selection_to_temp_list(preset_name, &files))
        .flatten();
    match list {
        Some(temp_file_path) => {
            cmd.arg("--input-files").arg(&temp_file_path);
        }
        None => {
            cmd.args(&files);
        }
    }

    cmd.spawn().map(|_| ()).map_err(|e| {
        dbg_log!(
            "Failed to spawn converter process {}: {:?}",
            bin_path.display(),
            e
        );
        E_FAIL.into()
    })
}

fn open_settings_window() -> Result<()> {
    let bin_path = get_bin_path();
    if !bin_path.exists() {
        dbg_log!(
            "Converter binary not found for settings: {}",
            bin_path.display()
        );
        return Err(E_FAIL.into());
    }
    Command::new(&bin_path)
        .arg("--settings")
        .spawn()
        .map(|_| ())
        .map_err(|e| {
            dbg_log!("Failed to open settings window: {:?}", e);
            E_FAIL.into()
        })
}

/// Inserts a string item (with optional sub-menu, icon and command id).
///
/// # Safety
/// `parent` must be a live menu handle.
unsafe fn insert_item(
    parent: HMENU,
    text: &str,
    id: Option<u32>,
    sub_menu: Option<HMENU>,
    bitmap: HBITMAP,
    enabled: bool,
) -> bool {
    let mut wide = wide_nul(text);
    let mut mii = MENUITEMINFOW {
        cbSize: std::mem::size_of::<MENUITEMINFOW>() as u32,
        fMask: MIIM_STRING | MIIM_FTYPE,
        fType: MFT_STRING,
        dwTypeData: PWSTR(wide.as_mut_ptr()),
        cch: (wide.len() - 1) as u32,
        ..Default::default()
    };
    if let Some(id) = id {
        mii.fMask |= MIIM_ID;
        mii.wID = id;
    }
    if let Some(sub) = sub_menu {
        mii.fMask |= MIIM_SUBMENU;
        mii.hSubMenu = sub;
    }
    if !bitmap.0.is_null() {
        mii.fMask |= MIIM_BITMAP;
        mii.hbmpItem = bitmap;
    }
    if !enabled {
        mii.fMask |= MIIM_STATE;
        mii.fState = MFS_DISABLED;
    }
    // SAFETY: `mii` and `wide` outlive the call; the caller guarantees `parent`.
    unsafe { InsertMenuItemW(parent, u32::MAX, true, &mii).is_ok() }
}

impl IContextMenu_Impl for FileConverterShellExt_Impl {
    fn QueryContextMenu(
        &self,
        hmenu: HMENU,
        indexmenu: u32,
        idcmdfirst: u32,
        idcmdlast: u32,
        uflags: u32,
    ) -> Result<()> {
        const CMF_DEFAULTONLY: u32 = 0x0001;
        // Returning MAKE_HRESULT(SEVERITY_SUCCESS, 0, 0): nothing added.
        let nothing_added = || Err(windows::core::Error::from_hresult(HRESULT(0)));

        if uflags & CMF_DEFAULTONLY != 0 {
            return nothing_added();
        }

        let entries = {
            let Ok(files) = self.selected_files.read() else {
                return nothing_added();
            };
            if files.is_empty() {
                return nothing_added();
            }
            plan_menu(&get_cached_settings().conversion_presets, &files)
        };
        if entries.is_empty() {
            return nothing_added();
        }

        // One id per preset, one for "Configure presets...", one for the
        // "File Converter" parent item.
        let presets_count = entries.len() as u32;
        let needed_count = presets_count + 2;

        // The shell reserves `[idcmdfirst, idcmdlast]` for this handler. If it
        // is not large enough we must add nothing at all: ids outside the
        // range collide with the next handler.
        let fits = idcmdfirst
            .checked_add(needed_count - 1)
            .is_some_and(|last_used| last_used <= idcmdlast);
        if !fits {
            return nothing_added();
        }

        let configure_cmd_id = idcmdfirst + presets_count;
        let parent_cmd_id = configure_cmd_id + 1;
        let icons = menu_bitmaps();
        let bmp = |raw: usize| HBITMAP(raw as *mut c_void);

        // SAFETY: every menu handle used below is created here and either
        // handed to the shell's menu or destroyed by the guard.
        unsafe {
            let h_sub_menu = CreatePopupMenu()?;
            // `DestroyMenu` on the error path destroys nested folder menus too;
            // without it the USER handles leak inside long-lived explorer.exe.
            let sub_menu_guard = scopeguard::guard(h_sub_menu, |h| {
                let _ = DestroyMenu(h);
            });

            // Folder path ("A/B") -> its popup menu.
            let mut folder_menus: Vec<(String, HMENU)> = Vec::new();

            for (i, entry) in entries.iter().enumerate() {
                let mut parent = h_sub_menu;
                let mut path = String::new();
                for folder in &entry.folders {
                    if folder.is_empty() {
                        continue;
                    }
                    if !path.is_empty() {
                        path.push('/');
                    }
                    path.push_str(folder);

                    if let Some((_, menu)) = folder_menus.iter().find(|(p, _)| *p == path) {
                        parent = *menu;
                        continue;
                    }
                    let Ok(folder_menu) = CreatePopupMenu() else {
                        break;
                    };
                    if insert_item(
                        parent,
                        folder,
                        None,
                        Some(folder_menu),
                        bmp(icons.folder),
                        true,
                    ) {
                        folder_menus.push((path.clone(), folder_menu));
                        parent = folder_menu;
                    } else {
                        let _ = DestroyMenu(folder_menu);
                        break;
                    }
                }

                insert_item(
                    parent,
                    &entry.label,
                    Some(idcmdfirst + i as u32),
                    None,
                    category_bitmap(entry.output_type),
                    entry.enabled,
                );
            }

            let sep_mii = MENUITEMINFOW {
                cbSize: std::mem::size_of::<MENUITEMINFOW>() as u32,
                fMask: MIIM_FTYPE,
                fType: MFT_SEPARATOR,
                ..Default::default()
            };
            let _ = InsertMenuItemW(h_sub_menu, u32::MAX, true, &sep_mii);
            insert_item(
                h_sub_menu,
                "Configure presets...",
                Some(configure_cmd_id),
                None,
                bmp(icons.settings),
                true,
            );

            let mut parent_text = wide_nul("File Converter");
            let mut parent_mii = MENUITEMINFOW {
                cbSize: std::mem::size_of::<MENUITEMINFOW>() as u32,
                fMask: MIIM_STRING | MIIM_SUBMENU | MIIM_ID | MIIM_FTYPE,
                fType: MFT_STRING,
                wID: parent_cmd_id,
                hSubMenu: h_sub_menu,
                dwTypeData: PWSTR(parent_text.as_mut_ptr()),
                cch: (parent_text.len() - 1) as u32,
                ..Default::default()
            };
            if icons.app != 0 {
                parent_mii.fMask |= MIIM_BITMAP;
                parent_mii.hbmpItem = bmp(icons.app);
            }

            if InsertMenuItemW(hmenu, indexmenu, true, &parent_mii).is_err() {
                return nothing_added();
            }
            // The shell's menu now owns the popup.
            let _ = scopeguard::ScopeGuard::into_inner(sub_menu_guard);
        }

        if let Ok(mut lock) = self.menu_entries.write() {
            *lock = entries;
        }

        // MAKE_HRESULT(SEVERITY_SUCCESS, 0, largest id offset + 1).
        Err(windows::core::Error::from_hresult(HRESULT(
            needed_count as i32,
        )))
    }

    fn InvokeCommand(&self, pici: *const CMINVOKECOMMANDINFO) -> Result<()> {
        if pici.is_null() {
            return Err(E_FAIL.into());
        }

        // SAFETY: `pici` was null-checked immediately before this block; the COM
        // runtime owns `CMINVOKECOMMANDINFO` for the duration of the call and it
        // is not mutated while we read `lpVerb`, so the field read is valid.
        let verb_val = unsafe { (*pici).lpVerb.0 as usize };
        if verb_val >> 16 != 0 {
            // A string verb: none of ours are addressed that way.
            return Err(E_FAIL.into());
        }
        let verb_offset = verb_val & 0xFFFF;

        let selected_files = self
            .selected_files
            .read()
            .map(|l| l.clone())
            .unwrap_or_default();
        let mut entries = self
            .menu_entries
            .read()
            .map(|l| l.clone())
            .unwrap_or_default();
        if entries.is_empty() {
            entries = plan_menu(&get_cached_settings().conversion_presets, &selected_files);
        }

        dbg_log!(
            "InvokeCommand verb_offset={} presets_count={}",
            verb_offset,
            entries.len()
        );

        match verb_offset.cmp(&entries.len()) {
            std::cmp::Ordering::Less => {
                let entry = &entries[verb_offset];
                if !entry.enabled {
                    return Err(E_FAIL.into());
                }
                launch_conversion(&entry.preset_name, &selected_files)
            }
            std::cmp::Ordering::Equal => open_settings_window(),
            std::cmp::Ordering::Greater => Ok(()),
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
        // Returning S_OK without filling the buffer handed Explorer whatever
        // was in it (it uses the canonical verb to de-duplicate items).
        Err(E_NOTIMPL.into())
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
    let wide = wide_nul(s);
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

/// Files of an `IShellItemArray`.
fn shell_item_array_files(items: Option<&IShellItemArray>) -> Vec<String> {
    let mut files = Vec::new();
    let Some(item_array) = items else {
        return files;
    };
    // SAFETY: standard IShellItemArray enumeration; every returned string is
    // freed with CoTaskMemFree after copying.
    unsafe {
        if let Ok(count) = item_array.GetCount() {
            for i in 0..count {
                let Ok(item) = item_array.GetItemAt(i) else {
                    continue;
                };
                let Ok(path_pwstr) = item.GetDisplayName(SIGDN_FILESYSPATH) else {
                    continue;
                };
                if path_pwstr.is_null() {
                    continue;
                }
                let path_str = path_pwstr.to_string().unwrap_or_default();
                CoTaskMemFree(Some(path_pwstr.0 as *const c_void));
                if !path_str.is_empty() {
                    files.push(path_str);
                }
            }
        }
    }
    files
}

// The extension is not registered as an `ExplorerCommandHandler` any more (a
// flat verb that opened the settings window instead of listing presets), but
// the interface stays implemented so hosts that ask for it get sensible
// answers: it opens the settings window with the selection staged.
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
        let mut files = shell_item_array_files(psiitemarray);
        if files.is_empty() {
            files = self
                .selected_files
                .read()
                .map(|l| l.clone())
                .unwrap_or_default();
        }
        let usable = !plan_menu(&get_cached_settings().conversion_presets, &files).is_empty()
            && get_bin_path().exists();
        if usable {
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
        let mut files = shell_item_array_files(psiitemarray);
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
        cmd.args(files);
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
        Err(E_NOTIMPL.into())
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
        dark_mode: None,
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

/// Full path of this DLL, if known.
fn module_path() -> Option<PathBuf> {
    let read = |h: HMODULE| -> Option<PathBuf> {
        let mut buf = vec![0u16; 1024];
        // SAFETY: `h` is a live module handle and `buf` a writable buffer
        // that outlives the call.
        let len = unsafe { GetModuleFileNameW(h, &mut buf) };
        (len > 0).then(|| wide_to_path(&buf))
    };

    let hinst = G_DLL_INSTANCE.load(Ordering::Relaxed);
    if hinst != 0 {
        if let Some(p) = read(HMODULE(hinst as *mut c_void)) {
            return Some(p);
        }
    }
    let dll_name = wide_nul("file_converter_shell.dll");
    // SAFETY: `dll_name` is NUL-terminated and outlives the call.
    let h = unsafe { GetModuleHandleW(PCWSTR(dll_name.as_ptr())) }.ok()?;
    read(h)
}

fn get_bin_path() -> PathBuf {
    use winreg::RegKey;
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};

    // The executable next to this DLL comes first: it is the copy this DLL
    // was built and registered with. A stale `AppPath` (from an older install
    // or a moved folder) used to win and launch the wrong version.
    if let Some(dll_path) = module_path() {
        if let Some(parent) = dll_path.parent() {
            let path = parent.join("file_converter_bin.exe");
            if path.exists() {
                return path;
            }
        }
    }

    for hive in [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE] {
        if let Ok(key) = RegKey::predef(hive).open_subkey("Software\\FileConverter") {
            if let Ok(app_path) = key.get_value::<String, _>("AppPath") {
                let path = PathBuf::from(&app_path);
                if path.exists() {
                    return path;
                }
            }
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
        // The null-out is ownership transfer to COM, which owns `ppvobject` for
        // the duration of the call; we only write, never read through it.
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
            // `fetch_sub` wraps on an unbalanced LockServer(FALSE), which would pin
            // the count at u32::MAX and make `DllCanUnloadNow` return S_FALSE
            // forever (the DLL could never unload).
            //
            // Implemented with `compare_exchange` rather than `fetch_update`
            // because that method was renamed in Rust 1.99 and using the old
            // spelling is a deprecation warning (an error under `-D warnings`).
            let current = G_LOCK_COUNT.load(Ordering::Relaxed);
            if current > 0 {
                let _ = G_LOCK_COUNT.compare_exchange(
                    current,
                    current - 1,
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                );
            }
        }
        Ok(())
    }
}

/// # Safety
///
/// COM entry point. The caller (the COM runtime) must supply valid,
/// non-null `rclsid` / `riid` GUID pointers and a non-null `ppv` out-pointer
/// for the class factory. All three are null-checked before any dereference.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn DllGetClassObject(
    rclsid: *const GUID,
    riid: *const GUID,
    ppv: *mut *mut c_void,
) -> HRESULT {
    if ppv.is_null() || rclsid.is_null() || riid.is_null() {
        return E_FAIL;
    }
    // SAFETY: the COM runtime owns `ppv` / `rclsid` / `riid` for the duration
    // of the call and they were null-checked above; this function only reads
    // the GUIDs and writes the out-pointer.
    unsafe {
        *ppv = std::ptr::null_mut();

        if *rclsid != CLSID_FILE_CONVERTER {
            return CLASS_E_CLASSNOTAVAILABLE;
        }
    }

    let factory: IClassFactory = FileConverterClassFactory.into();
    // SAFETY: `riid` and `ppv` are the caller-supplied out-parameters checked
    // non-null above, and `factory` is alive for the whole query.
    unsafe { factory.query(riid, ppv) }
}

/// # Safety
///
/// COM entry point. The caller must be the COM runtime only; the function
/// takes no pointers and only reads the atomic lock/object counters, so no
/// caller-owned memory is dereferenced.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn DllCanUnloadNow() -> HRESULT {
    if G_LOCK_COUNT.load(Ordering::Relaxed) == 0 && G_OBJECT_COUNT.load(Ordering::Relaxed) == 0 {
        S_OK
    } else {
        S_FALSE
    }
}

/// Removes every File Converter menu key under one `Software\Classes` root:
/// the CLSID, the context-menu and property-sheet handlers and the static
/// `shell\FileConverter` verbs older versions wrote. Those verbs were a flat
/// "File Converter" item that opened the settings window, shown next to the
/// real preset menu.
#[cfg(target_os = "windows")]
fn remove_menu_keys(root: &winreg::RegKey) {
    let _ = root.delete_subkey_all(format!("CLSID\\{CLSID_STR}"));
    for assoc in ALL_ASSOCIATIONS {
        for sub in [
            "shellex\\ContextMenuHandlers\\FileConverter",
            "shellex\\PropertySheetHandlers\\FileConverter",
            "shell\\FileConverter",
        ] {
            let _ = root.delete_subkey_all(format!("{assoc}\\{sub}"));
        }
    }
}

/// # Safety
///
/// COM entry point invoked by `regsvr32.exe` to register this DLL. The
/// caller must be the COM runtime only; the function takes no pointers,
/// so no caller-owned memory is dereferenced.
#[cfg(target_os = "windows")]
#[unsafe(no_mangle)]
pub unsafe extern "system" fn DllRegisterServer() -> HRESULT {
    use winreg::RegKey;
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_ALL_ACCESS};

    let Some(module_path) = module_path().filter(|p| !p.as_os_str().is_empty()) else {
        // Without the DLL's own path there is nothing meaningful to register.
        dbg_log!("Module path unknown; aborting registration");
        return E_FAIL;
    };
    let Some(parent) = module_path.parent() else {
        dbg_log!("Module path has no parent directory; aborting registration");
        return E_FAIL;
    };
    let mod_path_str = module_path.to_string_lossy().to_string();
    let bin_path_str = parent
        .join("file_converter_bin.exe")
        .to_string_lossy()
        .to_string();

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

    // HKLM\Software\Classes when elevated (system-wide), otherwise the
    // per-user HKCU\Software\Classes. Both are cleaned first: a per-user
    // CLSID left by an earlier non-elevated registration would otherwise
    // shadow the machine-wide one and keep loading an old DLL.
    let hklm_classes = RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey_with_flags("Software\\Classes", KEY_ALL_ACCESS)
        .ok();
    let hkcu_classes = RegKey::predef(HKEY_CURRENT_USER)
        .create_subkey("Software\\Classes")
        .map(|(k, _)| k)
        .ok();
    for root in [&hklm_classes, &hkcu_classes].into_iter().flatten() {
        remove_menu_keys(root);
    }
    let Some(root) = hklm_classes.as_ref().or(hkcu_classes.as_ref()) else {
        return E_FAIL;
    };

    // A local mutable flag rather than discarding every write error: regsvr32
    // previously reported success even when nothing could be written.
    let mut write_failed = false;
    let mut write = |path: &str, values: &[(&str, &str)]| match root.create_subkey(path) {
        Ok((key, _)) => {
            for (name, value) in values {
                if key.set_value(name, value).is_err() {
                    write_failed = true;
                }
            }
        }
        Err(e) => {
            dbg_log!("Failed to create {}: {:?}", path, e);
            write_failed = true;
        }
    };

    write(
        &format!("CLSID\\{CLSID_STR}"),
        &[("", "File Converter context menu")],
    );
    write(
        &format!("CLSID\\{CLSID_STR}\\InprocServer32"),
        &[("", &mod_path_str), ("ThreadingModel", "Apartment")],
    );
    // All files, like the C# original. Folders are deliberately left out: they
    // have no extension, so no preset applies to them.
    write(
        "*\\shellex\\ContextMenuHandlers\\FileConverter",
        &[("", CLSID_STR)],
    );

    if let Ok((key, _)) = RegKey::predef(HKEY_LOCAL_MACHINE)
        .create_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Shell Extensions\\Approved")
    {
        // Non-elevated registration cannot write HKLM; that is expected and not a
        // hard failure because the per-user CLSID registration above still works.
        let _ = key.set_value(CLSID_STR, &"File Converter context menu");
    }

    // SAFETY: `SHChangeNotify` takes no caller-owned pointers; the
    // `None` arguments map to valid nulls and the shell notification is
    // non-blocking and thread-safe.
    unsafe {
        SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None);
    }

    if write_failed {
        dbg_log!("Shell extension registration completed with errors");
        return E_FAIL;
    }

    S_OK
}

/// # Safety
///
/// COM entry point invoked by `regsvr32.exe` to unregister this DLL. The
/// caller must be the COM runtime only; the function takes no pointers,
/// so no caller-owned memory is dereferenced.
#[cfg(target_os = "windows")]
#[unsafe(no_mangle)]
pub unsafe extern "system" fn DllUnregisterServer() -> HRESULT {
    use winreg::RegKey;
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_ALL_ACCESS};

    for hive in [HKEY_LOCAL_MACHINE, HKEY_CURRENT_USER] {
        if let Ok(root) =
            RegKey::predef(hive).open_subkey_with_flags("Software\\Classes", KEY_ALL_ACCESS)
        {
            remove_menu_keys(&root);
        }
    }

    if let Ok(key) = RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey_with_flags(
        "Software\\Microsoft\\Windows\\CurrentVersion\\Shell Extensions\\Approved",
        KEY_ALL_ACCESS,
    ) {
        let _ = key.delete_value(CLSID_STR);
    }

    // SAFETY: `SHChangeNotify` takes no caller-owned pointers; the
    // `None` arguments map to valid nulls and the shell notification is
    // non-blocking and thread-safe.
    unsafe {
        SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None);
    }

    S_OK
}

#[cfg(test)]
mod tests {
    use super::*;
    use file_converter_core::types::InputPostConversionAction;

    fn preset(name: &str, output_type: OutputType, inputs: &[&str]) -> ConversionPreset {
        ConversionPreset {
            name: name.to_string(),
            output_type,
            output_file_name_template: "(p)(f)".to_string(),
            is_default_settings: true,
            input_types: inputs.iter().map(|s| (*s).into()).collect(),
            input_post_conversion_action: InputPostConversionAction::None,
            settings: vec![],
        }
    }

    fn files(paths: &[&str]) -> Vec<String> {
        paths.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn test_shell_preset_compatibility_filtering() {
        let preset = preset("To MP3", OutputType::Mp3, &["flac", "wav"]);
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
    fn test_pdf_gets_paged_png_with_folders() {
        let presets = [
            preset("To Png (paged)", OutputType::Png, &["pdf"]),
            preset("Scale 75%/To Png", OutputType::Png, &["pdf", "jpg"]),
            preset("To Mp3", OutputType::Mp3, &["flac"]),
            preset(
                "To Pdf (from Markdown/Typst/EPUB)",
                OutputType::Pdf,
                &["md", "pdf"],
            ),
        ];
        let plan = plan_menu(&presets, &files(&["C:\\Docs\\report.PDF"]));
        let labels: Vec<_> = plan.iter().map(|e| e.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "To Png (paged)",
                "To Png",
                "To Pdf (from Markdown/Typst/EPUB)"
            ]
        );
        assert_eq!(plan[1].folders, ["Scale 75%"]);
        // Slashes inside brackets are part of the name, not folders.
        assert!(plan[2].folders.is_empty());
        assert!(plan.iter().all(|e| e.enabled));
        assert_eq!(plan[1].preset_name, "Scale 75%/To Png");
    }

    #[test]
    fn test_mixed_selection_greys_out_partial_presets() {
        let presets = [
            preset("To Png", OutputType::Png, &["jpg", "pdf"]),
            preset("To Png (paged)", OutputType::Png, &["pdf"]),
            preset("To Mp3", OutputType::Mp3, &["flac"]),
        ];
        let plan = plan_menu(
            &presets,
            &files(&["C:\\a.pdf", "C:\\b.jpg", "C:\\c.JPG", "C:\\folder"]),
        );
        assert_eq!(plan.len(), 2);
        assert!(plan[0].enabled, "fits both pdf and jpg");
        assert!(!plan[1].enabled, "only fits the pdf");
    }

    #[test]
    fn test_nothing_offered_for_unknown_or_extensionless_items() {
        let presets = [preset("To Png", OutputType::Png, &["jpg"])];
        assert!(plan_menu(&presets, &files(&["C:\\Folder"])).is_empty());
        assert!(plan_menu(&presets, &files(&["C:\\notes.xyz"])).is_empty());
        assert!(plan_menu(&presets, &[]).is_empty());
    }

    #[test]
    fn test_shipped_defaults_offer_paged_png_for_pdf() {
        let plan = plan_menu(
            &DEFAULT_SETTINGS.conversion_presets,
            &files(&["C:\\Docs\\report.pdf"]),
        );
        assert!(
            plan.iter()
                .any(|e| e.preset_name == "To Png (paged)" && e.enabled),
            "{plan:?}"
        );
    }

    #[test]
    fn test_shell_category_icon_creation() {
        for t in [
            OutputType::Mp3,
            OutputType::Mp4,
            OutputType::Png,
            OutputType::Pdf,
            OutputType::Txt,
        ] {
            assert!(!category_bitmap(t).is_invalid());
        }
        // Cached: asking twice returns the same handle instead of a new one.
        assert_eq!(
            category_bitmap(OutputType::Png).0,
            category_bitmap(OutputType::Jpg).0
        );
    }
}
