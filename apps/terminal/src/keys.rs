//! Turns key presses into the bytes a terminal program expects.

use neo::{Key, KeyEvent};

/// xterm's modifier parameter: 1 plus shift 1, alt 2, ctrl 4.
fn modifier_param(k: &KeyEvent) -> u8 {
    1 + k.modifiers.shift as u8 + 2 * k.modifiers.alt as u8 + 4 * k.modifiers.ctrl as u8
}

/// Bytes for `k`, or `None` for keys the terminal ignores.
/// `app_cursor` is DECCKM, which switches arrows to SS3 sequences.
pub fn encode(k: &KeyEvent, app_cursor: bool) -> Option<Vec<u8>> {
    let m = modifier_param(k);
    let csi = |final_byte: char| -> Vec<u8> {
        if m > 1 { format!("\x1b[1;{m}{final_byte}").into_bytes() } else if app_cursor { format!("\x1bO{final_byte}").into_bytes() } else { format!("\x1b[{final_byte}").into_bytes() }
    };
    let tilde = |n: u8| -> Vec<u8> { if m > 1 { format!("\x1b[{n};{m}~").into_bytes() } else { format!("\x1b[{n}~").into_bytes() } };
    // Alt sends ESC first. On macOS Option types special characters instead.
    let meta = k.modifiers.alt && !cfg!(target_os = "macos");
    let with_meta = |mut b: Vec<u8>| {
        if meta {
            b.insert(0, 0x1b);
        }
        b
    };
    Some(match &k.key {
        Key::Enter => with_meta(vec![b'\r']),
        Key::Backspace => with_meta(if k.modifiers.ctrl { vec![0x08] } else { vec![0x7f] }),
        Key::Tab if k.modifiers.shift => b"\x1b[Z".to_vec(),
        Key::Tab => vec![b'\t'],
        Key::Escape => vec![0x1b],
        Key::Space if k.modifiers.ctrl => vec![0],
        Key::Space => with_meta(vec![b' ']),
        Key::Up => csi('A'),
        Key::Down => csi('B'),
        Key::Right => csi('C'),
        Key::Left => csi('D'),
        Key::Home => csi('H'),
        Key::End => csi('F'),
        Key::Delete => tilde(3),
        Key::PageUp => tilde(5),
        Key::PageDown => tilde(6),
        Key::Character(c) if k.modifiers.ctrl => {
            let ch = c.chars().next()?;
            let b = match ch {
                'a'..='z' => ch as u8 - b'a' + 1,
                '@' | '2' => 0,
                '[' | '3' => 0x1b,
                '\\' | '4' => 0x1c,
                ']' | '5' => 0x1d,
                '^' | '6' => 0x1e,
                '_' | '-' | '7' => 0x1f,
                '8' | '?' => 0x7f,
                _ => return k.text.clone().map(|t| with_meta(t.into_bytes())),
            };
            with_meta(vec![b])
        }
        Key::Character(c) => with_meta(k.text.clone().unwrap_or_else(|| c.clone()).into_bytes()),
        Key::Other => return k.text.clone().filter(|t| !t.chars().any(char::is_control)).map(String::into_bytes),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use neo::Modifiers;

    fn key(key: Key, text: Option<&str>, modifiers: Modifiers) -> KeyEvent {
        KeyEvent { key, pressed: true, repeat: false, modifiers, text: text.map(String::from) }
    }

    #[test]
    fn control_letters() {
        let ctrl = Modifiers { ctrl: true, ..Default::default() };
        assert_eq!(encode(&key(Key::Character("c".into()), Some("c"), ctrl), false), Some(vec![3]));
        assert_eq!(encode(&key(Key::Character("[".into()), None, ctrl), false), Some(vec![0x1b]));
    }

    #[test]
    fn arrows_follow_cursor_mode_and_modifiers() {
        let none = Modifiers::default();
        assert_eq!(encode(&key(Key::Up, None, none), false).unwrap(), b"\x1b[A");
        assert_eq!(encode(&key(Key::Up, None, none), true).unwrap(), b"\x1bOA");
        let ctrl = Modifiers { ctrl: true, ..Default::default() };
        assert_eq!(encode(&key(Key::Left, None, ctrl), true).unwrap(), b"\x1b[1;5D");
    }

    #[test]
    fn shifted_text_is_sent_as_typed() {
        let shift = Modifiers { shift: true, ..Default::default() };
        assert_eq!(encode(&key(Key::Character("4".into()), Some("$"), shift), false).unwrap(), b"$");
    }
}
