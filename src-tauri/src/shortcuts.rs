//! Configurable application shortcuts.
//!
//! The official frontend delegates shortcut matching to the native layer: the
//! settings panel reads and writes a shortcut table through
//! `get_app_shortcuts` / `set_app_shortcuts`, pauses matching through
//! `set_shortcut_capture`, and expects an `app-shortcut` event carrying the
//! action name. Linux matches the table against the same evdev stream that
//! already counts key presses, so the shortcuts keep working while another
//! window has focus.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};
use tauri::{AppHandle, Emitter, Manager, State};

// Binding modifiers are the same bit mask the official frontend uses for
// KeyboardEvent: Alt, Ctrl, Shift, Meta.
pub const ALT: u32 = 1;
pub const CTRL: u32 = 2;
pub const SHIFT: u32 = 4;
pub const META: u32 = 8;

pub const EV_LEFT_CTRL: u16 = 29;
pub const EV_LEFT_ALT: u16 = 56;
pub const EV_LEFT_SHIFT: u16 = 42;
pub const EV_RIGHT_SHIFT: u16 = 54;
pub const EV_RIGHT_CTRL: u16 = 97;
pub const EV_RIGHT_ALT: u16 = 100;
pub const EV_LEFT_META: u16 = 125;
pub const EV_RIGHT_META: u16 = 126;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Binding {
    pub key: u32,
    pub modifiers: u32,
}

impl Binding {
    const fn new(key: u32, modifiers: u32) -> Self {
        Self { key, modifiers }
    }

    fn is_set(self) -> bool {
        self.key != 0
    }
}

#[derive(Default)]
struct Inner {
    config: Mutex<BTreeMap<String, Binding>>,
    capture: AtomicBool,
    path: Mutex<Option<PathBuf>>,
}

/// Cheaply clonable handle shared with the input monitor thread.
#[derive(Clone, Default)]
pub struct Shortcuts(Arc<Inner>);

fn defaults() -> BTreeMap<String, Binding> {
    BTreeMap::from([
        ("togglePet".to_owned(), Binding::new(120, 0)), // F9
        ("gameMode".to_owned(), Binding::new(118, 0)),  // F7
        ("switchScreen".to_owned(), Binding::new(119, 0)), // F8
    ])
}

fn parse_config(config: &Value) -> Result<BTreeMap<String, Binding>, String> {
    let object = config.as_object().ok_or_else(|| "invalid".to_owned())?;
    let mut parsed = BTreeMap::new();
    for (action, value) in object {
        let entry = value.as_object().ok_or_else(|| "invalid".to_owned())?;
        let key = entry
            .get("key")
            .and_then(Value::as_u64)
            .filter(|key| *key <= 255)
            .ok_or_else(|| "invalid".to_owned())?;
        let modifiers = entry
            .get("modifiers")
            .and_then(Value::as_u64)
            .filter(|modifiers| *modifiers <= 15)
            .ok_or_else(|| "invalid".to_owned())?;
        parsed.insert(
            action.clone(),
            Binding {
                key: key as u32,
                modifiers: modifiers as u32,
            },
        );
    }
    // Two actions cannot own the same chord; the official UI reports this as
    // "setting_shortcut_duplicate".
    let mut seen = BTreeSet::new();
    for binding in parsed.values().filter(|binding| binding.is_set()) {
        if !seen.insert((binding.key, binding.modifiers)) {
            return Err("duplicate".to_owned());
        }
    }
    Ok(parsed)
}

impl Shortcuts {
    pub fn load(path: PathBuf) -> Self {
        let config = fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .and_then(|value| parse_config(&value).ok())
            .unwrap_or_else(defaults);
        Self(Arc::new(Inner {
            config: Mutex::new(config),
            capture: AtomicBool::new(false),
            path: Mutex::new(Some(path)),
        }))
    }

    pub fn value(&self) -> Value {
        let config = self
            .0
            .config
            .lock()
            .map(|config| config.clone())
            .unwrap_or_else(|poisoned| poisoned.into_inner().clone());
        serde_json::to_value(config).unwrap_or(Value::Null)
    }

    /// Returns the action whose chord matches, unless the settings panel is
    /// currently recording a new shortcut.
    pub fn action_for(&self, key: u32, modifiers: u32) -> Option<String> {
        if self.0.capture.load(Ordering::Acquire) || key == 0 {
            return None;
        }
        let config = self.0.config.lock().ok()?;
        config
            .iter()
            .find(|(_, binding)| binding.key == key && binding.modifiers == modifiers)
            .map(|(action, _)| action.clone())
    }

    pub fn set_capture(&self, active: bool) {
        self.0.capture.store(active, Ordering::Release);
    }

    pub fn apply(&self, config: &Value) -> Result<Value, String> {
        let parsed = parse_config(config)?;
        let serialized = serde_json::to_value(&parsed).map_err(|error| error.to_string())?;
        if let Ok(path) = self.0.path.lock() {
            if let Some(path) = path.as_ref() {
                persist(path, &serialized)?;
            }
        }
        let mut current = self
            .0
            .config
            .lock()
            .map_err(|_| "shortcut_config_lock_poisoned".to_owned())?;
        *current = parsed;
        Ok(serialized)
    }
}

fn persist(path: &Path, value: &Value) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("shortcut_config_write_failed: {error}"))?;
    }
    let temporary = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?;
    fs::write(&temporary, bytes)
        .map_err(|error| format!("shortcut_config_write_failed: {error}"))?;
    fs::rename(&temporary, path).map_err(|error| format!("shortcut_config_write_failed: {error}"))
}

/// Translates an evdev key code into the DOM `KeyboardEvent.keyCode` value the
/// official frontend stores in its shortcut table. Codes absent here cannot be
/// bound to an action.
pub fn dom_key_code(code: u16) -> Option<u32> {
    Some(match code {
        1 => 27,                                  // Escape
        2..=10 => 49 + u32::from(code - 2),       // 1..9
        11 => 48,                                 // 0
        12 => 189,                                // Minus
        13 => 187,                                // Equal
        14 => 8,                                  // Backspace
        15 => 9,                                  // Tab
        16 => 81,                                 // q
        17 => 87,                                 // w
        18 => 69,                                 // e
        19 => 82,                                 // r
        20 => 84,                                 // t
        21 => 89,                                 // y
        22 => 85,                                 // u
        23 => 73,                                 // i
        24 => 79,                                 // o
        25 => 80,                                 // p
        26 => 219,                                // [
        27 => 221,                                // ]
        28 => 13,                                 // Enter
        30 => 65,                                 // a
        31 => 83,                                 // s
        32 => 68,                                 // d
        33 => 70,                                 // f
        34 => 71,                                 // g
        35 => 72,                                 // h
        36 => 74,                                 // j
        37 => 75,                                 // k
        38 => 76,                                 // l
        39 => 186,                                // ;
        40 => 222,                                // '
        41 => 192,                                // `
        43 => 220,                                // backslash
        44 => 90,                                 // z
        45 => 88,                                 // x
        46 => 67,                                 // c
        47 => 86,                                 // v
        48 => 66,                                 // b
        49 => 78,                                 // n
        50 => 77,                                 // m
        51 => 188,                                // ,
        52 => 190,                                // .
        53 => 191,                                // /
        55 => 106,                                // Numpad *
        57 => 32,                                 // Space
        58 => 20,                                 // CapsLock
        59..=68 => 112 + u32::from(code - 59),    // F1..F10
        69 => 144,                                // NumLock
        70 => 145,                                // ScrollLock
        71 => 103,                                // Numpad 7
        72 => 104,                                // Numpad 8
        73 => 105,                                // Numpad 9
        74 => 109,                                // Numpad -
        75 => 100,                                // Numpad 4
        76 => 101,                                // Numpad 5
        77 => 102,                                // Numpad 6
        78 => 107,                                // Numpad +
        79 => 97,                                 // Numpad 1
        80 => 98,                                 // Numpad 2
        81 => 99,                                 // Numpad 3
        82 => 96,                                 // Numpad 0
        83 => 110,                                // Numpad .
        87 => 122,                                // F11
        88 => 123,                                // F12
        96 => 13,                                 // Numpad Enter
        98 => 111,                                // Numpad /
        99 => 44,                                 // PrintScreen
        102 => 36,                                // Home
        103 => 38,                                // ArrowUp
        104 => 33,                                // PageUp
        105 => 37,                                // ArrowLeft
        106 => 39,                                // ArrowRight
        107 => 35,                                // End
        108 => 40,                                // ArrowDown
        109 => 34,                                // PageDown
        110 => 45,                                // Insert
        111 => 46,                                // Delete
        113 => 173,                               // VolumeMute
        114 => 174,                               // VolumeDown
        115 => 175,                               // VolumeUp
        119 => 19,                                // Pause
        127 => 93,                                // ContextMenu
        183..=194 => 124 + u32::from(code - 183), // F13..F24
        _ => return None,
    })
}

pub fn is_modifier(code: u16) -> bool {
    matches!(
        code,
        EV_LEFT_CTRL
            | EV_RIGHT_CTRL
            | EV_LEFT_ALT
            | EV_RIGHT_ALT
            | EV_LEFT_SHIFT
            | EV_RIGHT_SHIFT
            | EV_LEFT_META
            | EV_RIGHT_META
    )
}

pub fn modifiers_from(held: &BTreeSet<u16>) -> u32 {
    let mut modifiers = 0;
    if held.contains(&EV_LEFT_ALT) || held.contains(&EV_RIGHT_ALT) {
        modifiers |= ALT;
    }
    if held.contains(&EV_LEFT_CTRL) || held.contains(&EV_RIGHT_CTRL) {
        modifiers |= CTRL;
    }
    if held.contains(&EV_LEFT_SHIFT) || held.contains(&EV_RIGHT_SHIFT) {
        modifiers |= SHIFT;
    }
    if held.contains(&EV_LEFT_META) || held.contains(&EV_RIGHT_META) {
        modifiers |= META;
    }
    modifiers
}

#[tauri::command]
pub fn get_app_shortcuts(shortcuts: State<'_, Shortcuts>) -> Value {
    shortcuts.value()
}

#[tauri::command]
pub fn set_app_shortcuts(
    app: AppHandle,
    shortcuts: State<'_, Shortcuts>,
    config: Value,
) -> Result<(), String> {
    let applied = shortcuts.apply(&config)?;
    // Other windows mirror the table to know whether F5 stays reserved for
    // devtools. The settings panel already updated itself from the invoke.
    let _ = app.emit("app-shortcuts-changed", applied);
    Ok(())
}

#[tauri::command]
pub fn set_shortcut_capture(shortcuts: State<'_, Shortcuts>, active: bool) -> Result<(), String> {
    shortcuts.set_capture(active);
    Ok(())
}

pub fn setup(app: &AppHandle) -> Result<(), String> {
    let path = app
        .path()
        .app_data_dir()
        .map_err(|error| format!("shortcut_config_dir_unavailable: {error}"))?
        .join("shortcuts.json");
    app.manage(Shortcuts::load(path));
    Ok(())
}

/// Resolves the shared table the input monitor uses for matching.
pub fn shared(app: &AppHandle) -> Shortcuts {
    app.state::<Shortcuts>().inner().clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn config(value: Value) -> BTreeMap<String, Binding> {
        parse_config(&value).expect("valid shortcut table")
    }

    #[test]
    fn defaults_match_the_official_f7_f8_f9_bindings() {
        let defaults = defaults();
        assert_eq!(defaults["gameMode"], Binding::new(118, 0));
        assert_eq!(defaults["switchScreen"], Binding::new(119, 0));
        assert_eq!(defaults["togglePet"], Binding::new(120, 0));
        for action in ["togglePet", "gameMode", "switchScreen"] {
            assert!(defaults.contains_key(action), "missing default {action}");
        }
    }

    #[test]
    fn evdev_codes_map_to_dom_key_codes() {
        assert_eq!(dom_key_code(65), Some(118)); // F7
        assert_eq!(dom_key_code(67), Some(120)); // F9
        assert_eq!(dom_key_code(30), Some(65)); // a
        assert_eq!(dom_key_code(2), Some(49)); // 1
        assert_eq!(dom_key_code(11), Some(48)); // 0
        assert_eq!(dom_key_code(28), Some(13)); // Enter
        assert_eq!(dom_key_code(57), Some(32)); // Space
        assert_eq!(dom_key_code(183), Some(124)); // F13
        assert_eq!(dom_key_code(194), Some(135)); // F24
        assert_eq!(dom_key_code(EV_LEFT_CTRL), None);
        assert_eq!(dom_key_code(0x120), None);
    }

    #[test]
    fn modifier_bits_match_keyboard_event_semantics() {
        let mut held = BTreeSet::new();
        assert_eq!(modifiers_from(&held), 0);
        held.insert(EV_LEFT_CTRL);
        held.insert(EV_RIGHT_SHIFT);
        held.insert(EV_RIGHT_ALT);
        held.insert(EV_LEFT_META);
        assert_eq!(modifiers_from(&held), CTRL | SHIFT | ALT | META);
        held.clear();
        held.insert(EV_LEFT_SHIFT);
        assert_eq!(modifiers_from(&held), SHIFT);
    }

    #[test]
    fn duplicate_chords_are_rejected_like_the_official_ui() {
        let duplicate = json!({
            "togglePet": { "key": 120, "modifiers": 0 },
            "gameMode": { "key": 120, "modifiers": 0 }
        });
        assert_eq!(parse_config(&duplicate).unwrap_err(), "duplicate");
        let distinct = json!({
            "togglePet": { "key": 120, "modifiers": 0 },
            "gameMode": { "key": 120, "modifiers": CTRL }
        });
        assert_eq!(config(distinct).len(), 2);
    }

    #[test]
    fn cleared_bindings_never_collide() {
        let cleared = json!({
            "togglePet": { "key": 0, "modifiers": 0 },
            "gameMode": { "key": 0, "modifiers": 0 },
            "switchScreen": { "key": 0, "modifiers": 0 }
        });
        let parsed = parse_config(&cleared).expect("cleared bindings are allowed");
        assert!(parsed.values().all(|binding| !binding.is_set()));
    }

    #[test]
    fn malformed_tables_fall_back_to_the_unavailable_error() {
        for value in [
            json!([]),
            json!({ "togglePet": true }),
            json!({ "togglePet": { "key": 300, "modifiers": 0 } }),
            json!({ "togglePet": { "key": 120, "modifiers": 99 } }),
        ] {
            assert_eq!(parse_config(&value).unwrap_err(), "invalid");
        }
    }

    #[test]
    fn matching_respects_capture_and_exact_modifiers() {
        let shortcuts = Shortcuts(Arc::new(Inner {
            config: Mutex::new(defaults()),
            capture: AtomicBool::new(false),
            path: Mutex::new(None),
        }));
        assert_eq!(shortcuts.action_for(120, 0).as_deref(), Some("togglePet"));
        assert_eq!(shortcuts.action_for(120, SHIFT), None);
        shortcuts.set_capture(true);
        assert_eq!(shortcuts.action_for(120, 0), None);
        shortcuts.set_capture(false);
        assert_eq!(shortcuts.action_for(120, 0).as_deref(), Some("togglePet"));
    }

    #[test]
    fn applying_a_table_persists_and_replaces_the_previous_one() {
        let directory = std::env::temp_dir().join(format!(
            "wuwa-shortcut-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let path = directory.join("shortcuts.json");
        let shortcuts = Shortcuts::load(path.clone());
        let applied = shortcuts
            .apply(&json!({ "togglePet": { "key": 121, "modifiers": CTRL } }))
            .expect("valid table");
        assert_eq!(applied["togglePet"]["key"], 121);
        assert_eq!(
            shortcuts.action_for(121, CTRL).as_deref(),
            Some("togglePet")
        );
        assert_eq!(shortcuts.action_for(120, 0), None);
        let persisted = serde_json::from_slice::<Value>(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(persisted["togglePet"]["key"], 121);
        assert_eq!(
            shortcuts.apply(&json!({ "togglePet": { "key": 1, "modifiers": 0 }, "gameMode": { "key": 1, "modifiers": 0 } })),
            Err("duplicate".to_owned())
        );
        assert_eq!(
            shortcuts.action_for(121, CTRL).as_deref(),
            Some("togglePet")
        );
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn stored_tables_survive_a_reload_and_invalid_files_do_not_panic() {
        let directory = std::env::temp_dir().join(format!(
            "wuwa-shortcut-reload-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let path = directory.join("shortcuts.json");
        fs::create_dir_all(&directory).unwrap();
        fs::write(&path, br#"{"togglePet":{"key":120,"modifiers":0}}"#).unwrap();
        let shortcuts = Shortcuts::load(path.clone());
        assert_eq!(shortcuts.action_for(120, 0).as_deref(), Some("togglePet"));
        fs::write(&path, b"not json").unwrap();
        let fallback = Shortcuts::load(path);
        assert_eq!(fallback.action_for(120, 0).as_deref(), Some("togglePet"));
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn unknown_actions_are_kept_so_a_newer_frontend_can_extend_the_table() {
        let parsed = config(json!({ "futureAction": { "key": 121, "modifiers": CTRL } }));
        assert!(parsed.contains_key("futureAction"));
        assert_eq!(parsed.len(), 1);
    }
}
