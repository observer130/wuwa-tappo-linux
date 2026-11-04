//! Layout-aware evdev key mapping.
//!
//! The official Windows backend runs through `rdev`, which maps Windows
//! virtual-key codes. Those are resolved through the active keyboard layout,
//! so a French AZERTY user pressing the key labelled "a" produces the same
//! DOM key code as a browser reports for that key. The Linux port originally
//! used a fixed US evdev table; this module resolves the key through
//! libxkbcommon instead, using the XKB rules currently installed on the
//! display.

use std::ffi::{c_char, c_int, c_void, CString};

#[repr(C)]
struct XkbRuleNames {
    rules: *const c_char,
    model: *const c_char,
    layout: *const c_char,
    variant: *const c_char,
    options: *const c_char,
}

#[link(name = "xkbcommon")]
unsafe extern "C" {
    fn xkb_context_new(flags: u32) -> *mut c_void;
    fn xkb_context_unref(context: *mut c_void);
    fn xkb_keymap_new_from_names(
        context: *mut c_void,
        names: *const XkbRuleNames,
        flags: u32,
    ) -> *mut c_void;
    fn xkb_keymap_unref(keymap: *mut c_void);
    fn xkb_keymap_key_get_syms_by_level(
        keymap: *mut c_void,
        keycode: u32,
        layout: u32,
        level: u32,
        syms_out: *mut *const u32,
    ) -> c_int;
    fn xkb_keysym_to_utf32(keysym: u32) -> u32;
}

pub struct KeyLayout {
    context: *mut c_void,
    keymap: *mut c_void,
}

impl KeyLayout {
    /// Builds a keymap from the active X11 XKB rules.
    ///
    /// Falls back to libxkbcommon's defaults when the display is unavailable
    /// or the rules property is missing.
    pub fn new() -> Option<Self> {
        let names = xkb_rules();
        let owned: Option<Vec<Option<CString>>> = names.as_ref().map(|values| {
            values
                .iter()
                .map(|value| {
                    if value.is_empty() {
                        None
                    } else {
                        CString::new(value.replace('\0', "")).ok()
                    }
                })
                .collect()
        });
        let rules = owned.as_ref().map(|owned| {
            let ptr = |value: &Option<CString>| {
                value
                    .as_ref()
                    .map_or(std::ptr::null(), |value| value.as_ptr())
            };
            XkbRuleNames {
                rules: ptr(&owned[0]),
                model: ptr(&owned[1]),
                layout: ptr(&owned[2]),
                variant: ptr(&owned[3]),
                options: ptr(&owned[4]),
            }
        });

        unsafe {
            let context = xkb_context_new(0);
            if context.is_null() {
                return None;
            }
            let keymap = xkb_keymap_new_from_names(
                context,
                rules
                    .as_ref()
                    .map_or(std::ptr::null(), |rules| rules as *const XkbRuleNames),
                0,
            );
            // An incomplete _XKB_RULES_NAMES property can make the named
            // keymap fail; libxkbcommon's defaults are a better fallback than
            // silently dropping back to the fixed US table.
            let keymap = if keymap.is_null() {
                xkb_keymap_new_from_names(context, std::ptr::null(), 0)
            } else {
                keymap
            };
            if keymap.is_null() {
                xkb_context_unref(context);
                return None;
            }
            Some(Self { context, keymap })
        }
    }

    fn keysym_at(&self, evdev_code: u16, level: u32) -> Option<u32> {
        // X11 keycodes are evdev keycodes plus the standard 8 offset.
        let keycode = u32::from(evdev_code).checked_add(8)?;
        let mut syms: *const u32 = std::ptr::null();
        let count =
            unsafe { xkb_keymap_key_get_syms_by_level(self.keymap, keycode, 0, level, &mut syms) };
        if count <= 0 || syms.is_null() {
            return None;
        }
        Some(unsafe { *syms })
    }

    /// Returns the character printed on the key's unshifted level.
    pub fn label(&self, evdev_code: u16) -> Option<String> {
        let keysym = self.keysym_at(evdev_code, 0)?;
        if is_keypad_keysym(keysym) {
            return None;
        }
        let character = char::from_u32(unsafe { xkb_keysym_to_utf32(keysym) })?;
        if character == ' ' {
            return Some(" ".into());
        }
        (!character.is_control()).then(|| character.to_string())
    }

    /// Returns the DOM `KeyboardEvent.keyCode` a browser would report.
    ///
    /// Level 0 is preferred for letters and digits. Layouts such as AZERTY
    /// place digits on level 1 and punctuation on level 0, so an alphanumeric
    /// level-1 symbol is preferred before falling back to punctuation.
    pub fn dom_key_code(&self, evdev_code: u16) -> Option<u32> {
        let first_keysym = self.keysym_at(evdev_code, 0);
        let second_keysym = self.keysym_at(evdev_code, 1);
        if first_keysym.is_some_and(is_keypad_keysym) || second_keysym.is_some_and(is_keypad_keysym)
        {
            // Numeric keypad keys have dedicated DOM key codes; the fallback
            // table in shortcuts.rs handles them exactly.
            return None;
        }
        let first = first_keysym.and_then(keysym_char);
        let second = second_keysym.and_then(keysym_char);
        if let Some(character) = first.filter(|value| value.is_ascii_alphanumeric()) {
            return char_to_dom_key_code(character);
        }
        if let Some(character) = second.filter(|value| value.is_ascii_alphanumeric()) {
            return char_to_dom_key_code(character);
        }
        first
            .and_then(char_to_dom_key_code)
            .or_else(|| second.and_then(char_to_dom_key_code))
    }
}

impl Drop for KeyLayout {
    fn drop(&mut self) {
        unsafe {
            xkb_keymap_unref(self.keymap);
            xkb_context_unref(self.context);
        }
    }
}

fn xkb_rules() -> Option<[String; 5]> {
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{AtomEnum, ConnectionExt};

    let (connection, screen) = x11rb::connect(None).ok()?;
    let root = connection.setup().roots.get(screen)?.root;
    let atom = connection
        .intern_atom(false, b"_XKB_RULES_NAMES")
        .ok()?
        .reply()
        .ok()?
        .atom;
    let value = connection
        .get_property(false, root, atom, AtomEnum::STRING, 0, 1024)
        .ok()?
        .reply()
        .ok()?
        .value;
    let mut fields: Vec<String> = value
        .split(|byte| *byte == 0)
        .map(|field| String::from_utf8_lossy(field).into_owned())
        .collect();
    fields.resize(5, String::new());
    fields.truncate(5);
    Some([
        fields[0].clone(),
        fields[1].clone(),
        fields[2].clone(),
        fields[3].clone(),
        fields[4].clone(),
    ])
}

fn keysym_char(keysym: u32) -> Option<char> {
    char::from_u32(unsafe { xkb_keysym_to_utf32(keysym) })
}

fn is_keypad_keysym(keysym: u32) -> bool {
    (0xff80..=0xffbd).contains(&keysym)
}

fn char_to_dom_key_code(character: char) -> Option<u32> {
    if character.is_ascii_alphabetic() {
        return Some(65 + u32::from(character.to_ascii_uppercase()) - 65);
    }
    if character.is_ascii_digit() {
        return Some(48 + u32::from(character) - 48);
    }
    Some(match character {
        ' ' => 32,
        ';' => 186,
        '=' => 187,
        ',' => 188,
        '-' => 189,
        '.' => 190,
        '/' => 191,
        '`' => 192,
        '[' => 219,
        '\\' => 220,
        ']' => 221,
        '\'' => 222,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_digits_and_punctuation_use_dom_key_codes() {
        assert_eq!(char_to_dom_key_code('a'), Some(65));
        assert_eq!(char_to_dom_key_code('Z'), Some(90));
        assert_eq!(char_to_dom_key_code('0'), Some(48));
        assert_eq!(char_to_dom_key_code('9'), Some(57));
        assert_eq!(char_to_dom_key_code(';'), Some(186));
        assert_eq!(char_to_dom_key_code(' '), Some(32));
        assert_eq!(char_to_dom_key_code('é'), None);
    }

    #[test]
    fn current_layout_maps_printable_keys_when_xkb_is_available() {
        if let Some(layout) = KeyLayout::new() {
            assert!(layout.label(30).is_some()); // KEY_A physical key
            assert!(layout.dom_key_code(30).is_some());
        }
    }
}
