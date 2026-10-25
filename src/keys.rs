//! Decodes key presses delivered by Moon's system keyboard (XTEST events on
//! this window) into password-field edits. Only the startup keyboard map is
//! queried; nothing typed is ever logged or stored outside the zeroizing
//! password buffer.

use std::error::Error;

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{ConnectionExt as _, KeyButMask, Keycode, Keysym};

const XK_BACK_SPACE: Keysym = 0xff08;
const XK_RETURN: Keysym = 0xff0d;
const XK_KP_ENTER: Keysym = 0xff8d;
const XK_MODE_SWITCH: Keysym = 0xff7e;
const XK_ISO_LEVEL3_SHIFT: Keysym = 0xfe03;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyInput {
    Character(char),
    Backspace,
    Enter,
}

pub struct KeyMap {
    min_keycode: Keycode,
    per_keycode: usize,
    keysyms: Vec<Keysym>,
    level3_mask: u16,
}

impl KeyMap {
    pub fn query<C: Connection>(conn: &C) -> Result<Self, Box<dyn Error>> {
        let setup = conn.setup();
        let (min, max) = (setup.min_keycode, setup.max_keycode);
        let count = max.saturating_sub(min).saturating_add(1);
        let keyboard = conn.get_keyboard_mapping(min, count)?.reply()?;
        let modifiers = conn.get_modifier_mapping()?.reply()?;
        let mut map = Self {
            min_keycode: min,
            per_keycode: usize::from(keyboard.keysyms_per_keycode),
            keysyms: keyboard.keysyms,
            level3_mask: 0,
        };
        let per_modifier = modifiers.keycodes.len() / 8;
        if per_modifier > 0 {
            for (index, keycodes) in modifiers.keycodes.chunks(per_modifier).enumerate() {
                if keycodes.iter().any(|keycode| {
                    *keycode != 0
                        && map
                            .symbols(*keycode)
                            .iter()
                            .any(|symbol| matches!(*symbol, XK_MODE_SWITCH | XK_ISO_LEVEL3_SHIFT))
                }) {
                    map.level3_mask |= 1 << index;
                }
            }
        }
        Ok(map)
    }

    fn symbols(&self, keycode: Keycode) -> &[Keysym] {
        let Some(index) = usize::from(keycode).checked_sub(usize::from(self.min_keycode)) else {
            return &[];
        };
        let start = index * self.per_keycode;
        self.keysyms
            .get(start..start + self.per_keycode)
            .unwrap_or(&[])
    }

    pub fn decode(&self, keycode: Keycode, state: KeyButMask) -> Option<KeyInput> {
        let state = u16::from(state);
        let shift = state & u16::from(KeyButMask::SHIFT) != 0;
        let level3 = self.level3_mask != 0 && state & self.level3_mask != 0;
        let symbols = self.symbols(keycode);
        let level = usize::from(shift) + 2 * usize::from(level3);
        let keysym = symbols
            .get(level)
            .copied()
            .filter(|symbol| *symbol != 0)
            .or_else(|| symbols.first().copied())?;
        decode_keysym(keysym, shift)
    }
}

fn decode_keysym(keysym: Keysym, shift: bool) -> Option<KeyInput> {
    match keysym {
        XK_BACK_SPACE => Some(KeyInput::Backspace),
        XK_RETURN | XK_KP_ENTER => Some(KeyInput::Enter),
        0x20..=0x7e => {
            let mut character = char::from_u32(keysym)?;
            // A keycode whose second level is absent still honors Shift.
            if shift && character.is_ascii_lowercase() {
                character.make_ascii_uppercase();
            }
            Some(KeyInput::Character(character))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map() -> KeyMap {
        KeyMap {
            min_keycode: 8,
            per_keycode: 2,
            // keycode 8: a/A, 9: 1/!, 10: BackSpace, 11: Return
            keysyms: vec![0x61, 0x41, 0x31, 0x21, XK_BACK_SPACE, 0, XK_RETURN, 0],
            level3_mask: 0,
        }
    }

    #[test]
    fn decodes_levels_and_editing_keys() {
        let map = map();
        assert_eq!(
            map.decode(8, KeyButMask::default()),
            Some(KeyInput::Character('a'))
        );
        assert_eq!(
            map.decode(8, KeyButMask::SHIFT),
            Some(KeyInput::Character('A'))
        );
        assert_eq!(
            map.decode(9, KeyButMask::SHIFT),
            Some(KeyInput::Character('!'))
        );
        assert_eq!(
            map.decode(10, KeyButMask::default()),
            Some(KeyInput::Backspace)
        );
        assert_eq!(map.decode(11, KeyButMask::SHIFT), Some(KeyInput::Enter));
        assert_eq!(map.decode(200, KeyButMask::default()), None);
    }
}
