//! The settings window: Presets, History, Preferences and About.
//!
//! All state lives in one [`State`] shared by the callbacks. Every edit marks
//! the window as having unsaved changes; nothing reaches the settings file
//! until Save (or Ctrl+S), and closing with unsaved changes asks first. The
//! theme is the one exception: it is a view preference, written immediately.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use file_converter_core::settings::{CompactStr, ConversionPreset, Settings};
use file_converter_core::types::{InputPostConversionAction, OutputType, preset_setting_keys};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};

use crate::settings_logic::{
    add_input_kind, history_matches, history_status, input_warning, join_preset_name,
    output_type_label, parse_input_types, preset_matches, preset_warning, sample_input,
    split_preset_name, validate_for_save,
};
use crate::{
    DEFAULT_SETTINGS_XML, HistoryItemData, HistoryRecord, PresetData, PresetSettingData,
    SettingsWindow, ask_yes_no, ask_yes_no_cancel, file_name_of, get_settings_paths,
    hw_accel_label, initialize_user_settings_if_needed, is_preset_compatible_with_file,
    is_windows_dark_mode, load_history, open_file_dialog, open_path_with_shell, open_url,
    parse_hw_accel_label, persist_theme_preference, preset_file_dialog,
    register_shell_extension_dll, reveal_in_explorer, save_history, show_error_box,
    spawn_conversion_process, unique_preset_name, unregister_shell_extension_dll,
};

/// Everything the callbacks share.
struct State {
    settings: RefCell<Settings>,
    xml_path: PathBuf,
    /// Files dropped or picked but not converted yet.
    pending: RefCell<Vec<String>>,
    history: RefCell<Vec<HistoryRecord>>,
}

// ---------------------------------------------------------------------------
// Small helpers
// ---------------------------------------------------------------------------

fn selected_idx(w: &SettingsWindow) -> usize {
    w.get_selected_preset_index().max(0) as usize
}

thread_local! {
    static STATUS_TIMER: slint::Timer = slint::Timer::default();
}

/// Shows a message as a toast that fades after a few seconds.
fn show_status(w: &SettingsWindow, msg: impl Into<SharedString>) {
    let msg: SharedString = msg.into();
    w.set_status_msg(msg.clone());
    let weak = w.as_weak();
    STATUS_TIMER.with(|t| {
        t.start(
            slint::TimerMode::SingleShot,
            Duration::from_secs(5),
            move || {
                if let Some(w) = weak.upgrade() {
                    if w.get_status_msg() == msg {
                        w.set_status_msg(SharedString::new());
                    }
                }
            },
        );
    });
}

fn theme_index(dark: Option<bool>) -> i32 {
    match dark {
        None => 0,
        Some(false) => 1,
        Some(true) => 2,
    }
}

fn string_model(items: Vec<String>) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(
        items
            .into_iter()
            .map(SharedString::from)
            .collect::<Vec<_>>(),
    ))
}

/// Validates a custom conversion-setting key typed into the editor.
///
/// Returns the trimmed key to store, or a user-facing message.
pub(crate) fn validate_custom_setting_key(
    key: &str,
    already_known: bool,
) -> Result<String, String> {
    let key = key.trim();
    if key.is_empty() {
        return Err("Enter a setting key first".to_string());
    }
    // Keys end up as XML attribute values and are matched case-sensitively by the
    // engines, so restrict them to the identifier characters the engines use.
    if !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err("Keys may only contain letters, digits and '_'".to_string());
    }
    if already_known {
        return Err("That key is already in the list above".to_string());
    }
    Ok(key.to_string())
}

// ---------------------------------------------------------------------------
// Model builders
// ---------------------------------------------------------------------------

fn preset_row(p: &ConversionPreset, i: usize) -> PresetData {
    let (folder, short) = split_preset_name(&p.name);
    PresetData {
        original_index: i as i32,
        name: p.name.as_str().into(),
        short_name: short.into(),
        folder: folder.replace('/', " / ").into(),
        category: p.output_type.kind().into(),
        output_ext: p.output_type.extension().to_uppercase().into(),
        input_types: p.input_types.join(", ").into(),
        is_default: p.is_default_settings,
    }
}

/// Rebuilds the preset list for the current search text and category chip.
fn refresh_list(w: &SettingsWindow, s: &Settings) {
    let query = w.get_search_query().trim().to_lowercase();
    let category = w.get_active_category();
    let rows: Vec<PresetData> = s
        .conversion_presets
        .iter()
        .enumerate()
        .filter(|(_, p)| preset_matches(p, &query, &category))
        .map(|(i, p)| preset_row(p, i))
        .collect();
    w.set_presets(ModelRc::new(VecModel::from(rows)));
    w.set_preset_total(s.conversion_presets.len() as i32);
}

/// Updates one row of the list in place (no flicker, keeps the scroll).
fn update_row(w: &SettingsWindow, s: &Settings, idx: usize) {
    let Some(p) = s.conversion_presets.get(idx) else {
        return;
    };
    let model = w.get_presets();
    for r in 0..model.row_count() {
        if model
            .row_data(r)
            .is_some_and(|row| row.original_index as usize == idx)
        {
            model.set_row_data(r, preset_row(p, idx));
            break;
        }
    }
}

/// The editable conversion settings of a preset. Every setting the engines
/// understand for this format is offered, plus any stored key without a
/// descriptor (shown as a removable "custom" row) so it is never silently lost.
fn setting_rows(preset: &ConversionPreset) -> Vec<PresetSettingData> {
    let descriptors = preset_setting_keys(preset.output_type);
    let mut rows = Vec::with_capacity(descriptors.len() + preset.settings.len());
    let mut previous_group = "";

    for d in &descriptors {
        let mut choices: Vec<String> = d.choices.iter().map(|c| c.to_string()).collect();
        // Long lists become a drop-down whose first entry means "default".
        if choices.len() > 4 {
            choices.insert(0, "Default".to_string());
        }
        rows.push(PresetSettingData {
            key: d.key.into(),
            label: d.label.into(),
            hint: d.hint.into(),
            group: d.group.into(),
            value: preset.get_setting_value(d.key).unwrap_or_default().into(),
            choices: string_model(choices),
            is_custom: false,
            is_group_start: d.group != previous_group,
        });
        previous_group = d.group;
    }

    let mut first_custom = true;
    for setting in &preset.settings {
        if descriptors.iter().any(|d| d.key == setting.key.as_str()) {
            continue;
        }
        rows.push(PresetSettingData {
            key: setting.key.as_str().into(),
            label: setting.key.as_str().into(),
            hint: "Custom setting".into(),
            group: "Custom".into(),
            value: setting.value.as_str().into(),
            choices: string_model(Vec::new()),
            is_custom: true,
            is_group_start: first_custom,
        });
        first_custom = false;
    }
    rows
}

fn refresh_setting_rows(w: &SettingsWindow, preset: &ConversionPreset) {
    w.set_edit_settings(ModelRc::new(VecModel::from(setting_rows(preset))));
}

/// Preview, warnings and the format badge: everything derived from the preset.
fn refresh_derived(w: &SettingsWindow, s: &Settings, idx: usize) {
    let Some(p) = s.conversion_presets.get(idx) else {
        return;
    };
    let input = sample_input(p.output_type);
    let preview = file_converter_core::path_helpers::generate_file_path_from_template(
        input,
        p.output_type.extension(),
        &p.output_file_name_template,
        1,
        1,
    );
    w.set_preview_input(input.into());
    w.set_preview_path(preview.into());
    w.set_preset_warning(preset_warning(s, idx).into());
    w.set_input_warning(input_warning(p).into());
    w.set_edit_category(p.output_type.kind().into());
    w.set_edit_ext(p.output_type.extension().to_uppercase().into());
}

/// Fills the editor for preset `idx` and marks it selected.
fn select(w: &SettingsWindow, s: &Settings, idx: usize) {
    let Some(p) = s.conversion_presets.get(idx) else {
        return;
    };
    let (folder, short) = split_preset_name(&p.name);
    w.set_selected_preset_index(idx as i32);
    w.set_edit_name(short.into());
    w.set_edit_folder(folder.into());
    w.set_edit_is_default(p.is_default_settings);
    w.set_edit_output_index(
        OutputType::ALL
            .iter()
            .position(|t| *t == p.output_type)
            .unwrap_or(0) as i32,
    );
    w.set_edit_input_types(p.input_types.join(", ").into());
    w.set_edit_template(p.output_file_name_template.as_str().into());
    w.set_edit_post_action(p.input_post_conversion_action.as_ref().into());
    w.set_new_setting_key(SharedString::new());
    w.set_new_setting_value(SharedString::new());
    w.set_settings_notice(SharedString::new());
    refresh_setting_rows(w, p);
    refresh_derived(w, s, idx);
}

fn populate_prefs(w: &SettingsWindow, s: &Settings) {
    w.set_dark_mode(s.dark_mode.unwrap_or_else(is_windows_dark_mode));
    w.set_theme_mode_index(theme_index(s.dark_mode));
    w.set_auto_start_on_file_drop(s.auto_start_on_file_drop);
    w.set_copy_files_in_clipboard_after_conversion(s.copy_files_in_clipboard_after_conversion);
    w.set_exit_application_when_conversions_finished(s.exit_application_when_conversions_finished);
    w.set_exit_delay_seconds(s.duration_between_end_of_conversions_and_application_exit);
    w.set_max_simultaneous_conversions(
        s.maximum_number_of_simultaneous_conversions
            .min(i32::MAX as usize) as i32,
    );
    w.set_cpu_count(
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(8)
            .max(s.maximum_number_of_simultaneous_conversions) as i32,
    );
    w.set_hw_accel_mode(hw_accel_label(s.hardware_acceleration_mode).into());
    w.set_check_upgrade_at_startup(s.check_upgrade_at_startup);
}

fn update_pending_ui(w: &SettingsWindow, st: &State) {
    let files = st.pending.borrow();
    w.set_pending_files_count(files.len() as i32);
    let summary = match files.len() {
        0 => String::new(),
        1 => file_name_of(&files[0]),
        n => format!("{} + {} more", file_name_of(&files[0]), n - 1),
    };
    w.set_pending_files_summary(summary.into());
}

fn refresh_history_view(w: &SettingsWindow, st: &State) {
    let query = w.get_history_query().trim().to_lowercase();
    let filter = w.get_history_filter();
    let history = st.history.borrow();
    let rows: Vec<HistoryItemData> = history
        .iter()
        .filter(|h| {
            history_matches(
                &h.preset_name,
                &h.input_path,
                &h.output_path,
                &h.status,
                &query,
                &filter,
            )
        })
        .map(|h| {
            let (status, message, ok) = history_status(&h.status);
            let first_output = h.output_path.split("; ").next().unwrap_or("").to_string();
            HistoryItemData {
                timestamp: h.timestamp.as_str().into(),
                preset_name: h.preset_name.as_str().into(),
                input_name: file_name_of(&h.input_path).into(),
                input_path: h.input_path.as_str().into(),
                output_name: if first_output.is_empty() {
                    SharedString::new()
                } else {
                    file_name_of(&first_output).into()
                },
                output_path: first_output.into(),
                status: status.into(),
                message: message.into(),
                ok,
            }
        })
        .collect();
    w.set_history_items(ModelRc::new(VecModel::from(rows)));
    w.set_history_total(history.len() as i32);
}

// ---------------------------------------------------------------------------
// Actions shared by several callbacks
// ---------------------------------------------------------------------------

/// Writes the settings file. Refuses (and shows the preset to fix) when a
/// preset would break the right-click menu.
fn save_now(w: &SettingsWindow, st: &State) -> bool {
    let invalid = validate_for_save(&st.settings.borrow()).err();
    if let Some((idx, msg)) = invalid {
        w.set_active_tab(0);
        w.set_search_query(SharedString::new());
        w.set_active_category("All".into());
        let s = st.settings.borrow();
        refresh_list(w, &s);
        select(w, &s, idx);
        show_status(w, format!("Not saved yet: {msg}"));
        return false;
    }
    let result = {
        let mut s = st.settings.borrow_mut();
        s.duration_between_end_of_conversions_and_application_exit = s
            .duration_between_end_of_conversions_and_application_exit
            .clamp(0.0, 10.0);
        s.save_to_file(&st.xml_path)
    };
    match result {
        Ok(()) => {
            w.set_dirty(false);
            show_status(w, "Settings saved");
            true
        }
        Err(e) => {
            show_status(w, format!("Could not save the settings: {e}"));
            false
        }
    }
}

/// Starts a conversion of `files` with the selected preset. The converter is
/// a separate process that reads the settings file, so unsaved edits are
/// saved first.
fn convert_files(w: &SettingsWindow, st: &State, files: Vec<String>) {
    if files.is_empty() {
        return;
    }
    if w.get_dirty() && !save_now(w, st) {
        st.pending.borrow_mut().extend(files);
        update_pending_ui(w, st);
        return;
    }
    let preset = {
        let s = st.settings.borrow();
        s.conversion_presets
            .get(selected_idx(w))
            .map(|p| (p.name.clone(), p.short_name().to_string()))
    };
    let Some((name, short)) = preset else {
        return;
    };
    let count = files.len();
    spawn_conversion_process(&name, &files);
    show_status(
        w,
        format!(
            "Converting {count} file{} with \"{short}\"…",
            if count == 1 { "" } else { "s" }
        ),
    );
}

/// Selects the first preset that can convert `file` when the selected one
/// can't. Returns the new preset's name when it switched.
fn select_compatible(w: &SettingsWindow, s: &Settings, file: &str) -> Option<String> {
    let current = selected_idx(w);
    if s.conversion_presets
        .get(current)
        .is_some_and(|p| is_preset_compatible_with_file(p, file))
    {
        return None;
    }
    let idx = s
        .conversion_presets
        .iter()
        .position(|p| is_preset_compatible_with_file(p, file))?;
    select(w, s, idx);
    Some(s.conversion_presets[idx].short_name().to_string())
}

/// Applies `edit` to the selected preset, then refreshes its row and the
/// derived editor state and marks the window dirty.
fn edit_selected(w: &SettingsWindow, st: &State, edit: impl FnOnce(&mut ConversionPreset)) {
    let idx = selected_idx(w);
    {
        let mut s = st.settings.borrow_mut();
        let Some(p) = s.conversion_presets.get_mut(idx) else {
            return;
        };
        edit(p);
    }
    let s = st.settings.borrow();
    update_row(w, &s, idx);
    refresh_derived(w, &s, idx);
    w.set_dirty(true);
}

fn load_settings_or_defaults() -> Settings {
    match initialize_user_settings_if_needed() {
        Ok(s) => s,
        Err(e) => {
            show_error_box(&format!(
                "{e}\n\nThe built-in presets are shown instead. Saving will replace the unreadable settings file."
            ));
            Settings::load_from_str(DEFAULT_SETTINGS_XML).unwrap_or_else(|_| Settings {
                serialization_version: Settings::CURRENT_VERSION,
                maximum_number_of_simultaneous_conversions: 0,
                exit_application_when_conversions_finished: true,
                duration_between_end_of_conversions_and_application_exit: 3.0,
                check_upgrade_at_startup: true,
                application_language_name: "en".to_string(),
                copy_files_in_clipboard_after_conversion: true,
                hardware_acceleration_mode: Default::default(),
                auto_start_on_file_drop: false,
                dark_mode: None,
                conversion_presets: vec![],
            })
        }
    }
}

fn check_for_updates_async(w: &SettingsWindow, announce_up_to_date: bool) {
    let weak = w.as_weak();
    std::thread::spawn(move || {
        let current = env!("CARGO_PKG_VERSION");
        let info = file_converter_core::update_check::check_for_updates(current);
        let _ = slint::invoke_from_event_loop(move || {
            let Some(w) = weak.upgrade() else {
                return;
            };
            match info {
                Some(info) if info.is_newer => {
                    w.set_update_available_text(
                        format!(
                            "Version {} is available (you have {current}).",
                            info.latest_version
                        )
                        .into(),
                    );
                    w.set_update_download_url(info.download_url.into());
                }
                Some(_) if announce_up_to_date => {
                    show_status(&w, format!("You have the latest version ({current})."));
                }
                None if announce_up_to_date => {
                    show_status(
                        &w,
                        "Couldn't reach the update server. Check your connection.",
                    );
                }
                _ => {}
            }
        });
    });
}

// ---------------------------------------------------------------------------
// Window
// ---------------------------------------------------------------------------

pub(crate) fn run(initial_files: Option<Vec<String>>) {
    let window = match SettingsWindow::new() {
        Ok(w) => w,
        Err(e) => {
            show_error_box(&format!("The settings window could not be opened: {e}"));
            return;
        }
    };

    let settings = load_settings_or_defaults();
    let check_upgrade = settings.check_upgrade_at_startup;
    let state = Rc::new(State {
        settings: RefCell::new(settings),
        xml_path: get_settings_paths().1,
        pending: RefCell::new(initial_files.unwrap_or_default()),
        history: RefCell::new(load_history()),
    });

    // ---- initial population
    {
        let s = state.settings.borrow();
        window.set_app_version(env!("CARGO_PKG_VERSION").into());
        window.set_project_url(env!("CARGO_PKG_REPOSITORY").into());
        window.set_settings_path(state.xml_path.display().to_string().into());
        window.set_output_type_labels(string_model(
            OutputType::ALL
                .iter()
                .map(|t| output_type_label(*t))
                .collect(),
        ));
        populate_prefs(&window, &s);
        refresh_list(&window, &s);

        let first_file = state.pending.borrow().first().cloned();
        let idx = first_file
            .and_then(|f| {
                s.conversion_presets
                    .iter()
                    .position(|p| is_preset_compatible_with_file(p, &f))
            })
            .unwrap_or(0);
        select(&window, &s, idx);
    }
    refresh_history_view(&window, &state);
    update_pending_ui(&window, &state);

    wire_presets(&window, &state);
    wire_editor(&window, &state);
    wire_files(&window, &state);
    wire_preferences(&window, &state);
    wire_history(&window, &state);
    wire_window(&window, &state);

    if check_upgrade {
        check_for_updates_async(&window, false);
    }

    let _ = window.run();
}

/// Save / discard, theme, closing with unsaved changes.
fn wire_window(window: &SettingsWindow, state: &Rc<State>) {
    {
        let (weak, st) = (window.as_weak(), state.clone());
        window.on_save_settings(move || {
            if let Some(w) = weak.upgrade() {
                if w.get_dirty() {
                    save_now(&w, &st);
                } else {
                    show_status(&w, "Nothing to save");
                }
            }
        });
    }
    {
        let (weak, st) = (window.as_weak(), state.clone());
        window.on_discard_changes(move || {
            let Some(w) = weak.upgrade() else {
                return;
            };
            if !ask_yes_no("Discard all changes since the last save?") {
                return;
            }
            let fresh = load_settings_or_defaults();
            let idx = selected_idx(&w).min(fresh.conversion_presets.len().saturating_sub(1));
            *st.settings.borrow_mut() = fresh;
            let s = st.settings.borrow();
            populate_prefs(&w, &s);
            refresh_list(&w, &s);
            select(&w, &s, idx);
            w.set_dirty(false);
            show_status(&w, "Changes discarded");
        });
    }
    {
        let (weak, st) = (window.as_weak(), state.clone());
        window.on_toggle_theme(move |dark| {
            st.settings.borrow_mut().dark_mode = Some(dark);
            if let Some(w) = weak.upgrade() {
                w.set_theme_mode_index(theme_index(Some(dark)));
            }
            persist_theme_preference(Some(dark));
        });
    }
    {
        let (weak, st) = (window.as_weak(), state.clone());
        window.on_set_theme_mode(move |index| {
            let choice = match index {
                1 => Some(false),
                2 => Some(true),
                _ => None,
            };
            st.settings.borrow_mut().dark_mode = choice;
            if let Some(w) = weak.upgrade() {
                w.set_dark_mode(choice.unwrap_or_else(is_windows_dark_mode));
            }
            persist_theme_preference(choice);
        });
    }
    window.on_open_update_url(|url| open_url(&url));
    {
        let weak = window.as_weak();
        window.on_check_updates_now(move || {
            if let Some(w) = weak.upgrade() {
                show_status(&w, "Checking for updates…");
                check_for_updates_async(&w, true);
            }
        });
    }
    {
        let weak = window.as_weak();
        window.on_register_shell(move || {
            if let Some(w) = weak.upgrade() {
                show_status(&w, register_shell_extension_dll());
            }
        });
    }
    {
        let weak = window.as_weak();
        window.on_unregister_shell(move || {
            if let Some(w) = weak.upgrade() {
                show_status(&w, unregister_shell_extension_dll());
            }
        });
    }
    {
        let st = state.clone();
        window.on_open_settings_folder(move || {
            if let Some(dir) = st.xml_path.parent() {
                open_path_with_shell(&dir.to_string_lossy());
            }
        });
    }
    {
        let (weak, st) = (window.as_weak(), state.clone());
        window.window().on_close_requested(move || {
            let Some(w) = weak.upgrade() else {
                return slint::CloseRequestResponse::HideWindow;
            };
            if !w.get_dirty() {
                return slint::CloseRequestResponse::HideWindow;
            }
            match ask_yes_no_cancel("Save your changes before closing?") {
                Some(true) if save_now(&w, &st) => slint::CloseRequestResponse::HideWindow,
                Some(false) => slint::CloseRequestResponse::HideWindow,
                _ => slint::CloseRequestResponse::KeepWindowShown,
            }
        });
    }
}

/// The preset list: search, filter, select, add, duplicate, delete, reorder,
/// import and export.
fn wire_presets(window: &SettingsWindow, state: &Rc<State>) {
    {
        let (weak, st) = (window.as_weak(), state.clone());
        window.on_search_query_changed(move |_| {
            if let Some(w) = weak.upgrade() {
                refresh_list(&w, &st.settings.borrow());
            }
        });
    }
    {
        let (weak, st) = (window.as_weak(), state.clone());
        window.on_filter_category_changed(move |_| {
            if let Some(w) = weak.upgrade() {
                refresh_list(&w, &st.settings.borrow());
            }
        });
    }
    {
        let (weak, st) = (window.as_weak(), state.clone());
        window.on_select_preset(move |index| {
            if let Some(w) = weak.upgrade() {
                select(&w, &st.settings.borrow(), index.max(0) as usize);
            }
        });
    }
    {
        let (weak, st) = (window.as_weak(), state.clone());
        window.on_new_preset(move || {
            let Some(w) = weak.upgrade() else {
                return;
            };
            let idx = {
                let mut s = st.settings.borrow_mut();
                let name = unique_preset_name(&s, "New preset");
                s.conversion_presets.push(ConversionPreset {
                    name,
                    output_type: OutputType::Png,
                    is_default_settings: false,
                    input_types: vec![CompactStr::new("jpg"), CompactStr::new("bmp")],
                    input_post_conversion_action: InputPostConversionAction::None,
                    settings: vec![],
                    output_file_name_template: "(p)(f)".to_string(),
                });
                s.conversion_presets.len() - 1
            };
            // Clear the filters so the new preset is visible.
            w.set_search_query(SharedString::new());
            w.set_active_category("All".into());
            let s = st.settings.borrow();
            refresh_list(&w, &s);
            select(&w, &s, idx);
            w.set_dirty(true);
            show_status(&w, "New preset added. Give it a name and pick a format.");
        });
    }
    {
        let (weak, st) = (window.as_weak(), state.clone());
        window.on_duplicate_preset(move |index| {
            let Some(w) = weak.upgrade() else {
                return;
            };
            let idx = index.max(0) as usize;
            let new_idx = {
                let mut s = st.settings.borrow_mut();
                let Some(original) = s.conversion_presets.get(idx) else {
                    return;
                };
                let mut copy = original.clone();
                copy.name = unique_preset_name(&s, &format!("{} (copy)", copy.name));
                copy.is_default_settings = false;
                // Right after the original, so it lands next to it in the menu.
                s.conversion_presets.insert(idx + 1, copy);
                idx + 1
            };
            let s = st.settings.borrow();
            refresh_list(&w, &s);
            select(&w, &s, new_idx);
            w.set_dirty(true);
            show_status(&w, "Preset duplicated");
        });
    }
    {
        let (weak, st) = (window.as_weak(), state.clone());
        window.on_delete_preset(move |index| {
            let Some(w) = weak.upgrade() else {
                return;
            };
            let idx = index.max(0) as usize;
            let (count, name) = {
                let s = st.settings.borrow();
                let Some(p) = s.conversion_presets.get(idx) else {
                    return;
                };
                (s.conversion_presets.len(), p.name.clone())
            };
            if count <= 1 {
                show_status(&w, "The last preset can't be deleted.");
                return;
            }
            if !ask_yes_no(&format!("Delete the preset \"{name}\"?")) {
                return;
            }
            let mut s = st.settings.borrow_mut();
            s.conversion_presets.remove(idx);
            let next = idx.min(s.conversion_presets.len() - 1);
            drop(s);
            let s = st.settings.borrow();
            refresh_list(&w, &s);
            select(&w, &s, next);
            w.set_dirty(true);
            show_status(&w, format!("Deleted \"{name}\""));
        });
    }
    {
        let (weak, st) = (window.as_weak(), state.clone());
        window.on_move_preset(move |delta| {
            let Some(w) = weak.upgrade() else {
                return;
            };
            let idx = selected_idx(&w);
            let target = idx as i64 + i64::from(delta);
            {
                let mut s = st.settings.borrow_mut();
                if target < 0 || target as usize >= s.conversion_presets.len() {
                    return;
                }
                s.conversion_presets.swap(idx, target as usize);
            }
            let s = st.settings.borrow();
            refresh_list(&w, &s);
            select(&w, &s, target as usize);
            w.set_dirty(true);
        });
    }
    {
        let (weak, st) = (window.as_weak(), state.clone());
        window.on_import_presets(move || {
            let Some(w) = weak.upgrade() else {
                return;
            };
            let Some(path) = preset_file_dialog(false, "") else {
                return;
            };
            match file_converter_core::settings::load_presets_file(&path) {
                Ok(presets) if !presets.is_empty() => {
                    let count = presets.len();
                    let first_new = {
                        let mut s = st.settings.borrow_mut();
                        let first_new = s.conversion_presets.len();
                        for mut preset in presets {
                            preset.name = unique_preset_name(&s, &preset.name);
                            preset.is_default_settings = false;
                            s.conversion_presets.push(preset);
                        }
                        first_new
                    };
                    w.set_search_query(SharedString::new());
                    w.set_active_category("All".into());
                    let s = st.settings.borrow();
                    refresh_list(&w, &s);
                    select(&w, &s, first_new);
                    w.set_dirty(true);
                    show_status(
                        &w,
                        format!("Imported {count} preset(s). Save to keep them."),
                    );
                }
                Ok(_) => show_status(&w, "That file contains no presets."),
                Err(e) => show_status(&w, format!("Could not read presets: {e}")),
            }
        });
    }
    {
        let (weak, st) = (window.as_weak(), state.clone());
        window.on_export_presets(move || {
            let Some(w) = weak.upgrade() else {
                return;
            };
            let shown: Vec<usize> = {
                let model = w.get_presets();
                (0..model.row_count())
                    .filter_map(|i| model.row_data(i))
                    .filter_map(|row| usize::try_from(row.original_index).ok())
                    .collect()
            };
            let presets: Vec<ConversionPreset> = {
                let s = st.settings.borrow();
                shown
                    .iter()
                    .filter_map(|&i| s.conversion_presets.get(i).cloned())
                    .collect()
            };
            if presets.is_empty() {
                show_status(&w, "No presets to export.");
                return;
            }
            let Some(path) = preset_file_dialog(true, "FileConverter presets.xml") else {
                return;
            };
            match file_converter_core::settings::save_presets_file(&path, &presets) {
                Ok(()) => show_status(
                    &w,
                    format!(
                        "Exported {} preset(s) to {}",
                        presets.len(),
                        file_name_of(&path)
                    ),
                ),
                Err(e) => show_status(&w, format!("Could not export presets: {e}")),
            }
        });
    }
    {
        let (weak, st) = (window.as_weak(), state.clone());
        window.on_restore_default_presets(move || {
            let Some(w) = weak.upgrade() else {
                return;
            };
            if !ask_yes_no(
                "Add back the built-in presets that are missing?\n\nYour own presets and changes are kept.",
            ) {
                return;
            }
            let Ok(defaults) = Settings::load_from_str(DEFAULT_SETTINGS_XML) else {
                return;
            };
            let added = {
                let mut s = st.settings.borrow_mut();
                let before = s.conversion_presets.len();
                s.merge(defaults);
                s.conversion_presets.len() - before
            };
            if added == 0 {
                show_status(&w, "All built-in presets are already there.");
                return;
            }
            let s = st.settings.borrow();
            refresh_list(&w, &s);
            w.set_dirty(true);
            show_status(&w, format!("Added {added} built-in preset(s). Save to keep them."));
        });
    }
}

/// The preset editor on the right.
fn wire_editor(window: &SettingsWindow, state: &Rc<State>) {
    {
        let (weak, st) = (window.as_weak(), state.clone());
        window.on_preset_field_changed(move || {
            let Some(w) = weak.upgrade() else {
                return;
            };
            let name = join_preset_name(&w.get_edit_folder(), &w.get_edit_name());
            let template = w.get_edit_template().to_string();
            let inputs = parse_input_types(&w.get_edit_input_types());
            edit_selected(&w, &st, |p| {
                p.name = name;
                p.output_file_name_template = template;
                p.input_types = inputs;
            });
        });
    }
    {
        let (weak, st) = (window.as_weak(), state.clone());
        window.on_set_output_type_index(move |index| {
            let Some(w) = weak.upgrade() else {
                return;
            };
            let Some(&output) = OutputType::ALL.get(index.max(0) as usize) else {
                return;
            };
            edit_selected(&w, &st, |p| p.output_type = output);
            // Which settings apply depends on the format.
            let s = st.settings.borrow();
            if let Some(p) = s.conversion_presets.get(selected_idx(&w)) {
                refresh_setting_rows(&w, p);
            }
        });
    }
    {
        let (weak, st) = (window.as_weak(), state.clone());
        window.on_add_input_category(move |kind| {
            let Some(w) = weak.upgrade() else {
                return;
            };
            let mut added = 0;
            edit_selected(&w, &st, |p| {
                added = add_input_kind(&mut p.input_types, &kind, p.output_type);
            });
            let s = st.settings.borrow();
            if let Some(p) = s.conversion_presets.get(selected_idx(&w)) {
                w.set_edit_input_types(p.input_types.join(", ").into());
                if added == 0 {
                    show_status(
                        &w,
                        format!(
                            "No {} types were added: they are already listed or can't become {}.",
                            kind.to_lowercase(),
                            p.output_type.extension().to_uppercase()
                        ),
                    );
                }
            }
        });
    }
    {
        let (weak, st) = (window.as_weak(), state.clone());
        window.on_clear_input_types(move || {
            if let Some(w) = weak.upgrade() {
                w.set_edit_input_types(SharedString::new());
                edit_selected(&w, &st, |p| p.input_types.clear());
            }
        });
    }
    {
        let (weak, st) = (window.as_weak(), state.clone());
        window.on_set_post_action(move |action| {
            let Some(w) = weak.upgrade() else {
                return;
            };
            if let Ok(parsed) = action.as_str().parse::<InputPostConversionAction>() {
                w.set_edit_post_action(action.clone());
                edit_selected(&w, &st, |p| p.input_post_conversion_action = parsed);
            }
        });
    }
    {
        let (weak, st) = (window.as_weak(), state.clone());
        window.on_set_path_template(move |template| {
            if let Some(w) = weak.upgrade() {
                w.set_edit_template(template.clone());
                edit_selected(&w, &st, |p| {
                    p.output_file_name_template = template.to_string()
                });
            }
        });
    }
    {
        let (weak, st) = (window.as_weak(), state.clone());
        window.on_set_preset_setting(move |key, value| {
            let Some(w) = weak.upgrade() else {
                return;
            };
            let trimmed = value.trim().to_string();
            edit_selected(&w, &st, |p| {
                // Empty means "use the engine default": drop the key.
                if trimmed.is_empty() {
                    p.remove_setting(&key);
                } else {
                    p.set_setting_value(&key, &trimmed);
                }
            });
            // Only choice rows need a redraw (to move the highlight); redrawing
            // free-text rows on every keystroke would steal the caret.
            let s = st.settings.borrow();
            if let Some(p) = s.conversion_presets.get(selected_idx(&w)) {
                let has_choices = preset_setting_keys(p.output_type)
                    .iter()
                    .any(|d| d.key == key.as_str() && !d.choices.is_empty());
                if has_choices {
                    refresh_setting_rows(&w, p);
                }
            }
        });
    }
    {
        let (weak, st) = (window.as_weak(), state.clone());
        window.on_remove_preset_setting(move |key| {
            let Some(w) = weak.upgrade() else {
                return;
            };
            let mut removed = false;
            edit_selected(&w, &st, |p| removed = p.remove_setting(&key));
            let s = st.settings.borrow();
            if let Some(p) = s.conversion_presets.get(selected_idx(&w)) {
                refresh_setting_rows(&w, p);
            }
            w.set_settings_notice_ok(removed);
            w.set_settings_notice(if removed {
                format!("Removed {key}").into()
            } else {
                format!("{key} was not set").into()
            });
        });
    }
    {
        let (weak, st) = (window.as_weak(), state.clone());
        window.on_add_preset_setting(move |key, value| {
            let Some(w) = weak.upgrade() else {
                return;
            };
            let already_known = {
                let s = st.settings.borrow();
                s.conversion_presets.get(selected_idx(&w)).is_some_and(|p| {
                    preset_setting_keys(p.output_type)
                        .iter()
                        .any(|d| d.key == key.trim())
                })
            };
            let key = match validate_custom_setting_key(&key, already_known) {
                Ok(k) => k,
                Err(message) => {
                    w.set_settings_notice_ok(false);
                    w.set_settings_notice(message.into());
                    return;
                }
            };
            edit_selected(&w, &st, |p| p.set_setting_value(&key, value.trim()));
            let s = st.settings.borrow();
            if let Some(p) = s.conversion_presets.get(selected_idx(&w)) {
                refresh_setting_rows(&w, p);
            }
            w.set_new_setting_key(SharedString::new());
            w.set_new_setting_value(SharedString::new());
            w.set_settings_notice_ok(true);
            w.set_settings_notice(format!("Added {key}").into());
        });
    }
}

/// Dropping, picking and converting files.
fn wire_files(window: &SettingsWindow, state: &Rc<State>) {
    {
        let (weak, st) = (window.as_weak(), state.clone());
        window.on_native_files_dropped(move |dropped| {
            let Some(w) = weak.upgrade() else {
                return;
            };
            let files: Vec<String> = dropped.iter().map(|f| f.to_string()).collect();
            let Some(first) = files.first().cloned() else {
                return;
            };
            // Pick a preset that can actually convert what was dropped.
            let switched = select_compatible(&w, &st.settings.borrow(), &first);
            if let Some(name) = &switched {
                show_status(
                    &w,
                    format!("Selected \"{name}\", which can convert these files."),
                );
            }
            st.pending.borrow_mut().extend(files);
            if st.settings.borrow().auto_start_on_file_drop {
                let all = std::mem::take(&mut *st.pending.borrow_mut());
                update_pending_ui(&w, &st);
                convert_files(&w, &st, all);
            } else {
                update_pending_ui(&w, &st);
            }
        });
    }

    #[cfg(target_os = "windows")]
    {
        let weak = window.as_weak();
        crate::setup_window_drag_and_drop(move |dropped| {
            let weak = weak.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(w) = weak.upgrade() {
                    let model: Vec<SharedString> = dropped.into_iter().map(Into::into).collect();
                    w.invoke_native_files_dropped(ModelRc::new(VecModel::from(model)));
                }
            });
        });
    }

    {
        // "Convert files…": pick, then convert straight away.
        let (weak, st) = (window.as_weak(), state.clone());
        window.on_select_files(move || {
            let picked = open_file_dialog();
            if picked.is_empty() {
                return;
            }
            if let Some(w) = weak.upgrade() {
                convert_files(&w, &st, picked);
            }
        });
    }
    {
        let (weak, st) = (window.as_weak(), state.clone());
        window.on_start_pending_conversion(move || {
            if let Some(w) = weak.upgrade() {
                let files = std::mem::take(&mut *st.pending.borrow_mut());
                update_pending_ui(&w, &st);
                convert_files(&w, &st, files);
            }
        });
    }
    {
        let (weak, st) = (window.as_weak(), state.clone());
        window.on_clear_pending_files(move || {
            if let Some(w) = weak.upgrade() {
                st.pending.borrow_mut().clear();
                update_pending_ui(&w, &st);
            }
        });
    }
}

/// Preferences page. Every change is kept in memory and marks the window
/// dirty, so Save / Discard work the same way as for presets.
fn wire_preferences(window: &SettingsWindow, state: &Rc<State>) {
    fn pref<T: 'static>(
        window: &SettingsWindow,
        state: &Rc<State>,
        apply: impl Fn(&mut Settings, T) + 'static,
    ) -> impl Fn(T) + 'static {
        let (weak, st) = (window.as_weak(), state.clone());
        move |value| {
            apply(&mut st.settings.borrow_mut(), value);
            if let Some(w) = weak.upgrade() {
                w.set_dirty(true);
            }
        }
    }

    window.on_toggle_auto_start(pref(window, state, |s, v| s.auto_start_on_file_drop = v));
    window.on_toggle_copy_clipboard(pref(window, state, |s, v| {
        s.copy_files_in_clipboard_after_conversion = v;
    }));
    window.on_toggle_auto_close(pref(window, state, |s, v| {
        s.exit_application_when_conversions_finished = v;
    }));
    window.on_set_exit_delay(pref(window, state, |s, v: i32| {
        s.duration_between_end_of_conversions_and_application_exit = v.clamp(0, 10) as f32;
    }));
    window.on_set_max_conversions(pref(window, state, |s, v: i32| {
        s.maximum_number_of_simultaneous_conversions = v.max(0) as usize;
    }));
    window.on_set_hw_accel(pref(window, state, |s, v: SharedString| {
        s.hardware_acceleration_mode = parse_hw_accel_label(&v);
    }));
    window.on_toggle_check_upgrade(pref(window, state, |s, v| s.check_upgrade_at_startup = v));
}

/// History page.
fn wire_history(window: &SettingsWindow, state: &Rc<State>) {
    {
        let (weak, st) = (window.as_weak(), state.clone());
        window.on_refresh_history(move || {
            *st.history.borrow_mut() = load_history();
            if let Some(w) = weak.upgrade() {
                refresh_history_view(&w, &st);
            }
        });
    }
    {
        let (weak, st) = (window.as_weak(), state.clone());
        window.on_history_query_changed(move |_| {
            if let Some(w) = weak.upgrade() {
                refresh_history_view(&w, &st);
            }
        });
    }
    {
        let (weak, st) = (window.as_weak(), state.clone());
        window.on_history_filter_changed(move |_| {
            if let Some(w) = weak.upgrade() {
                refresh_history_view(&w, &st);
            }
        });
    }
    {
        let (weak, st) = (window.as_weak(), state.clone());
        window.on_clear_history(move || {
            let Some(w) = weak.upgrade() else {
                return;
            };
            if !ask_yes_no("Clear the conversion history?\n\nConverted files are not touched.") {
                return;
            }
            save_history(&[]);
            st.history.borrow_mut().clear();
            refresh_history_view(&w, &st);
            show_status(&w, "History cleared");
        });
    }
    {
        let weak = window.as_weak();
        window.on_open_path(move |path| {
            if Path::new(path.as_str()).exists() {
                open_path_with_shell(&path);
            } else if let Some(w) = weak.upgrade() {
                show_status(&w, "That file no longer exists.");
            }
        });
    }
    {
        let weak = window.as_weak();
        window.on_reveal_path(move |path| {
            if !reveal_in_explorer(&path) {
                if let Some(w) = weak.upgrade() {
                    show_status(&w, "That file and its folder no longer exist.");
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_custom_setting_key() {
        // Accepted: identifier-shaped keys, trimmed.
        assert_eq!(
            validate_custom_setting_key("AudioNormalize", false).unwrap(),
            "AudioNormalize"
        );
        assert_eq!(
            validate_custom_setting_key("  My_Key2  ", false).unwrap(),
            "My_Key2"
        );

        // Rejected: empty / whitespace only.
        assert!(validate_custom_setting_key("", false).is_err());
        assert!(validate_custom_setting_key("   ", false).is_err());

        // Rejected: characters that would break XML attribute values or the
        // engines' case-sensitive key lookups.
        for bad in [
            "Audio Bitrate",  // space in the middle
            "Audio/Bitrate",  // path separator
            "Audio\nBitrate", // newline would inject XML
            "<Key>",          // markup
            "Key=1",          // attribute injection
            "kéy",            // non-ASCII
        ] {
            assert!(
                validate_custom_setting_key(bad, false).is_err(),
                "{bad:?} should have been rejected"
            );
        }

        // A key that already has a descriptor must not be added a second time.
        assert!(validate_custom_setting_key("VideoQuality", true).is_err());
        assert!(validate_custom_setting_key("VideoQuality", false).is_ok());
    }

    #[test]
    fn long_choice_lists_get_a_default_entry() {
        let preset = ConversionPreset {
            name: "To Mp4".into(),
            output_type: OutputType::Mp4,
            is_default_settings: false,
            input_types: vec![],
            input_post_conversion_action: InputPostConversionAction::None,
            settings: vec![],
            output_file_name_template: "(p)(f)".into(),
        };
        let rows = setting_rows(&preset);
        let speed = rows
            .iter()
            .find(|r| r.key == "VideoEncodingSpeed")
            .expect("encoder speed row");
        assert_eq!(speed.choices.row_data(0).as_deref(), Some("Default"));
        assert_eq!(speed.choices.row_count(), 10);
        let audio = rows
            .iter()
            .find(|r| r.key == "EnableAudio")
            .expect("keep audio row");
        assert_eq!(audio.choices.row_count(), 2, "short lists stay chips");
    }

    #[test]
    fn theme_index_maps_all_choices() {
        assert_eq!(theme_index(None), 0);
        assert_eq!(theme_index(Some(false)), 1);
        assert_eq!(theme_index(Some(true)), 2);
    }
}
