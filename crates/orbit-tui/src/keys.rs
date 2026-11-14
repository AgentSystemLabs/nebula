//! Encode crossterm key events into the byte sequences a PTY child expects.
//!
//! Two dialects: conventional xterm-style encoding for legacy children
//! (shells, vim without extras), and kitty-keyboard-protocol CSI-u encoding
//! when the child has pushed enhancement flags (Claude Code does — that's
//! how Cmd/Option combos and Shift+Enter reach it). The daemon tracks the
//! child's flag stack and tells us via `ServerEvent::KittyFlags`.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Kitty flag bit: keys that would be ambiguous get CSI-u encodings.
const DISAMBIGUATE: u8 = 0x1;
/// Kitty flag bit: ALL keys are reported as escape codes, even plain text.
const REPORT_ALL: u8 = 0x8;

pub fn encode_key(key: &KeyEvent, kitty_flags: u8) -> Option<Vec<u8>> {
    if kitty_flags & DISAMBIGUATE != 0 {
        encode_kitty(key, kitty_flags)
    } else {
        encode_legacy(key)
    }
}

// ---- kitty CSI-u dialect ----

/// Kitty modifier bitmask (shift=1, alt=2, ctrl=4, super=8, hyper=16, meta=32).
fn kitty_mods(mods: KeyModifiers) -> u8 {
    let mut bits = 0u8;
    if mods.contains(KeyModifiers::SHIFT) {
        bits |= 1;
    }
    if mods.contains(KeyModifiers::ALT) {
        bits |= 2;
    }
    if mods.contains(KeyModifiers::CONTROL) {
        bits |= 4;
    }
    if mods.contains(KeyModifiers::SUPER) {
        bits |= 8;
    }
    if mods.contains(KeyModifiers::HYPER) {
        bits |= 16;
    }
    if mods.contains(KeyModifiers::META) {
        bits |= 32;
    }
    bits
}

/// `CSI <cp>u` or `CSI <cp>;<mods+1>u`.
fn csi_u(cp: u32, bits: u8) -> Vec<u8> {
    if bits == 0 {
        format!("\x1b[{cp}u").into_bytes()
    } else {
        format!("\x1b[{cp};{}u", bits as u32 + 1).into_bytes()
    }
}

/// Functional keys keep their legacy final byte, gaining a modifier field:
/// `CSI 1;<mods+1><f>` (arrows/Home/End) — plain form `CSI <f>`.
fn csi_func(final_byte: char, bits: u8) -> Vec<u8> {
    if bits == 0 {
        format!("\x1b[{final_byte}").into_bytes()
    } else {
        format!("\x1b[1;{}{final_byte}", bits as u32 + 1).into_bytes()
    }
}

/// `CSI <n>~` / `CSI <n>;<mods+1>~` (PageUp/Down, Del, Ins, F5+).
fn csi_tilde(n: u8, bits: u8) -> Vec<u8> {
    if bits == 0 {
        format!("\x1b[{n}~").into_bytes()
    } else {
        format!("\x1b[{n};{}~", bits as u32 + 1).into_bytes()
    }
}

fn encode_kitty(key: &KeyEvent, flags: u8) -> Option<Vec<u8>> {
    let mut bits = kitty_mods(key.modifiers);
    let report_all = flags & REPORT_ALL != 0;

    let seq = match key.code {
        KeyCode::Char(c) => {
            // The key field is the lowercase codepoint; shift rides the
            // modifier bits.
            let base = if c.is_uppercase() {
                bits |= 1;
                c.to_lowercase().next().unwrap_or(c)
            } else {
                c
            };
            if bits & !1 == 0 && !report_all {
                // Unmodified (or shift-only) text stays plain text.
                let mut buf = [0u8; 4];
                c.encode_utf8(&mut buf).as_bytes().to_vec()
            } else {
                csi_u(base as u32, bits)
            }
        }
        // Esc is the poster child for disambiguation: always CSI-u.
        KeyCode::Esc => csi_u(27, bits),
        KeyCode::Enter if bits == 0 && !report_all => b"\r".to_vec(),
        KeyCode::Enter => csi_u(13, bits),
        KeyCode::Tab if bits == 0 && !report_all => b"\t".to_vec(),
        KeyCode::Tab => csi_u(9, bits),
        KeyCode::BackTab => csi_u(9, bits | 1),
        KeyCode::Backspace if bits == 0 && !report_all => vec![0x7f],
        KeyCode::Backspace => csi_u(127, bits),
        KeyCode::Up => csi_func('A', bits),
        KeyCode::Down => csi_func('B', bits),
        KeyCode::Right => csi_func('C', bits),
        KeyCode::Left => csi_func('D', bits),
        KeyCode::Home => csi_func('H', bits),
        KeyCode::End => csi_func('F', bits),
        KeyCode::Insert => csi_tilde(2, bits),
        KeyCode::Delete => csi_tilde(3, bits),
        KeyCode::PageUp => csi_tilde(5, bits),
        KeyCode::PageDown => csi_tilde(6, bits),
        KeyCode::F(n @ 1..=4) if bits == 0 => {
            vec![0x1b, b'O', b'P' + (n - 1)]
        }
        KeyCode::F(n @ 1..=4) => {
            format!("\x1b[1;{}{}", bits as u32 + 1, (b'P' + (n - 1)) as char).into_bytes()
        }
        KeyCode::F(n) => csi_tilde(fkey_tilde_num(n)?, bits),
        _ => return None,
    };
    Some(seq)
}

// ---- legacy xterm dialect ----

fn encode_legacy(key: &KeyEvent) -> Option<Vec<u8>> {
    // Cmd & friends have no legacy encoding at all — swallowing beats leaking
    // the bare character (Cmd+c must not type 'c' into the shell).
    if key
        .modifiers
        .intersects(KeyModifiers::SUPER | KeyModifiers::HYPER | KeyModifiers::META)
    {
        return None;
    }
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);

    let mut out: Vec<u8> = Vec::with_capacity(8);
    if alt {
        out.push(0x1b);
    }

    match key.code {
        KeyCode::Char(c) => {
            if ctrl {
                // Ctrl+A..Z → 0x01..0x1a; Ctrl+Space → NUL; a few punctuation ctrls.
                let b = match c.to_ascii_lowercase() {
                    ch @ 'a'..='z' => (ch as u8) - b'a' + 1,
                    ' ' | '@' => 0,
                    '[' => 0x1b,
                    '\\' => 0x1c,
                    ']' => 0x1d,
                    '^' => 0x1e,
                    '_' | '/' => 0x1f,
                    _ => return None,
                };
                out.push(b);
            } else {
                let mut buf = [0u8; 4];
                out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            }
        }
        KeyCode::Enter => out.push(b'\r'),
        KeyCode::Tab => out.push(b'\t'),
        KeyCode::BackTab => out.extend_from_slice(b"\x1b[Z"),
        KeyCode::Backspace => out.push(0x7f),
        KeyCode::Esc => out.push(0x1b),
        KeyCode::Up => out.extend_from_slice(arrow(b'A', key.modifiers)),
        KeyCode::Down => out.extend_from_slice(arrow(b'B', key.modifiers)),
        KeyCode::Right => out.extend_from_slice(arrow(b'C', key.modifiers)),
        KeyCode::Left => out.extend_from_slice(arrow(b'D', key.modifiers)),
        KeyCode::Home => out.extend_from_slice(b"\x1b[H"),
        KeyCode::End => out.extend_from_slice(b"\x1b[F"),
        KeyCode::PageUp => out.extend_from_slice(b"\x1b[5~"),
        KeyCode::PageDown => out.extend_from_slice(b"\x1b[6~"),
        KeyCode::Delete => out.extend_from_slice(b"\x1b[3~"),
        KeyCode::Insert => out.extend_from_slice(b"\x1b[2~"),
        KeyCode::F(n) => out.extend_from_slice(&fkey(n)?),
        _ => return None,
    }
    Some(out)
}

/// Arrow keys, with xterm modifier encoding (\x1b[1;<m><dir>) when modified.
fn arrow(dir: u8, mods: KeyModifiers) -> &'static [u8] {
    // Static tables keep this allocation-free for the common cases.
    macro_rules! seq {
        ($d:literal) => {
            match (
                mods.contains(KeyModifiers::SHIFT),
                mods.contains(KeyModifiers::ALT),
                mods.contains(KeyModifiers::CONTROL),
            ) {
                (false, false, false) => concat!("\x1b[", $d).as_bytes(),
                (true, false, false) => concat!("\x1b[1;2", $d).as_bytes(),
                (false, true, false) => concat!("\x1b[1;3", $d).as_bytes(),
                (true, true, false) => concat!("\x1b[1;4", $d).as_bytes(),
                (false, false, true) => concat!("\x1b[1;5", $d).as_bytes(),
                (true, false, true) => concat!("\x1b[1;6", $d).as_bytes(),
                (false, true, true) => concat!("\x1b[1;7", $d).as_bytes(),
                (true, true, true) => concat!("\x1b[1;8", $d).as_bytes(),
            }
        };
    }
    match dir {
        b'A' => seq!("A"),
        b'B' => seq!("B"),
        b'C' => seq!("C"),
        _ => seq!("D"),
    }
}

fn fkey_tilde_num(n: u8) -> Option<u8> {
    Some(match n {
        5 => 15,
        6 => 17,
        7 => 18,
        8 => 19,
        9 => 20,
        10 => 21,
        11 => 23,
        12 => 24,
        _ => return None,
    })
}

fn fkey(n: u8) -> Option<Vec<u8>> {
    let seq: &[u8] = match n {
        1 => b"\x1bOP",
        2 => b"\x1bOQ",
        3 => b"\x1bOR",
        4 => b"\x1bOS",
        _ => return Some(format!("\x1b[{}~", fkey_tilde_num(n)?).into_bytes()),
    };
    Some(seq.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    #[test]
    fn plain_char() {
        assert_eq!(
            encode_key(&key(KeyCode::Char('a'), KeyModifiers::NONE), 0),
            Some(b"a".to_vec())
        );
    }

    #[test]
    fn ctrl_c() {
        assert_eq!(
            encode_key(&key(KeyCode::Char('c'), KeyModifiers::CONTROL), 0),
            Some(vec![0x03])
        );
    }

    #[test]
    fn alt_char_prefixes_esc() {
        assert_eq!(
            encode_key(&key(KeyCode::Char('f'), KeyModifiers::ALT), 0),
            Some(vec![0x1b, b'f'])
        );
    }

    #[test]
    fn arrows() {
        assert_eq!(
            encode_key(&key(KeyCode::Up, KeyModifiers::NONE), 0),
            Some(b"\x1b[A".to_vec())
        );
        assert_eq!(
            encode_key(&key(KeyCode::Left, KeyModifiers::CONTROL), 0),
            Some(b"\x1b[1;5D".to_vec())
        );
    }

    #[test]
    fn utf8_char() {
        assert_eq!(
            encode_key(&key(KeyCode::Char('é'), KeyModifiers::NONE), 0),
            Some("é".as_bytes().to_vec())
        );
    }

    #[test]
    fn legacy_swallows_super_combos() {
        assert_eq!(
            encode_key(&key(KeyCode::Char('c'), KeyModifiers::SUPER), 0),
            None
        );
        assert_eq!(
            encode_key(&key(KeyCode::Backspace, KeyModifiers::SUPER), 0),
            None
        );
    }

    // ---- kitty dialect (flags=1: disambiguate) ----

    #[test]
    fn kitty_plain_text_stays_text() {
        assert_eq!(
            encode_key(&key(KeyCode::Char('a'), KeyModifiers::NONE), 1),
            Some(b"a".to_vec())
        );
        assert_eq!(
            encode_key(&key(KeyCode::Char('A'), KeyModifiers::SHIFT), 1),
            Some(b"A".to_vec())
        );
        assert_eq!(
            encode_key(&key(KeyCode::Enter, KeyModifiers::NONE), 1),
            Some(b"\r".to_vec())
        );
    }

    #[test]
    fn kitty_option_p() {
        assert_eq!(
            encode_key(&key(KeyCode::Char('p'), KeyModifiers::ALT), 1),
            Some(b"\x1b[112;3u".to_vec())
        );
    }

    #[test]
    fn kitty_cmd_backspace() {
        assert_eq!(
            encode_key(&key(KeyCode::Backspace, KeyModifiers::SUPER), 1),
            Some(b"\x1b[127;9u".to_vec())
        );
    }

    #[test]
    fn kitty_shift_enter_and_esc() {
        assert_eq!(
            encode_key(&key(KeyCode::Enter, KeyModifiers::SHIFT), 1),
            Some(b"\x1b[13;2u".to_vec())
        );
        assert_eq!(
            encode_key(&key(KeyCode::Esc, KeyModifiers::NONE), 1),
            Some(b"\x1b[27u".to_vec())
        );
    }

    #[test]
    fn kitty_ctrl_char_uses_csi_u() {
        assert_eq!(
            encode_key(&key(KeyCode::Char('c'), KeyModifiers::CONTROL), 1),
            Some(b"\x1b[99;5u".to_vec())
        );
    }

    #[test]
    fn kitty_uppercase_sets_shift_bit() {
        assert_eq!(
            encode_key(
                &key(
                    KeyCode::Char('P'),
                    KeyModifiers::CONTROL | KeyModifiers::SHIFT
                ),
                1
            ),
            Some(b"\x1b[112;6u".to_vec())
        );
    }

    #[test]
    fn kitty_arrows_keep_legacy_form() {
        assert_eq!(
            encode_key(&key(KeyCode::Up, KeyModifiers::NONE), 1),
            Some(b"\x1b[A".to_vec())
        );
        assert_eq!(
            encode_key(&key(KeyCode::Up, KeyModifiers::SUPER), 1),
            Some(b"\x1b[1;9A".to_vec())
        );
    }

    #[test]
    fn kitty_report_all_escapes_plain_keys() {
        // flags = disambiguate | report-all (0x9)
        assert_eq!(
            encode_key(&key(KeyCode::Char('a'), KeyModifiers::NONE), 0x9),
            Some(b"\x1b[97u".to_vec())
        );
        assert_eq!(
            encode_key(&key(KeyCode::Enter, KeyModifiers::NONE), 0x9),
            Some(b"\x1b[13u".to_vec())
        );
    }
}
