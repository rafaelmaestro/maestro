use gpui_kit::Keystroke;

/// Ctrl+Shift+letra/dígito/espaço e os binds de split do ghostty/config são do Maestro, nunca do PTY.
/// Ctrl+Shift+- fica de fora: é o ^_ (desfazer) do readline e do nano.
pub fn is_app_shortcut(k: &Keystroke) -> bool {
    let m = &k.modifiers;
    // Alt+N: nova feature, como no protótipo; Alt+1…6: os modos do título; Alt+7: nota nova; Alt+T/W/[/]: abas de trabalho
    (m.alt
        && !m.control
        && !m.shift
        && matches!(
            k.key.as_str(),
            "n" | "1" | "2" | "3" | "4" | "5" | "6" | "7" | "t" | "w" | "[" | "]"
        ))
        || feature_digit(k).is_some()
        || m.control
            && m.shift
            && !m.alt
            && (k.key.len() == 1 && k.key.as_bytes()[0].is_ascii_alphanumeric()
                || matches!(
                    k.key.as_str(),
                    "enter" | "space" | "\\" | "up" | "down" | "left" | "right"
                ))
}

/// Ctrl+Shift+1…9. No X11 o GPUI entrega o símbolo do layout US sem o Shift (Ctrl+`!` em vez de Ctrl+Shift+1).
pub fn feature_digit(k: &Keystroke) -> Option<usize> {
    let m = &k.modifiers;
    if !m.control || m.alt || k.key.len() != 1 {
        return None;
    }
    let i = match "123456789".find(&k.key) {
        Some(i) if m.shift => i,
        Some(_) => return None,
        None => "!@#$%^&*(".find(&k.key)?,
    };
    Some(i + 1)
}

pub fn to_bytes(k: &Keystroke, app_cursor: bool) -> Option<Vec<u8>> {
    let m = &k.modifiers;
    let arrow = |c: u8| {
        Some(if app_cursor {
            vec![0x1b, b'O', c]
        } else {
            vec![0x1b, b'[', c]
        })
    };
    let named: Option<Vec<u8>> = match k.key.as_str() {
        // Ctrl/Shift/Alt+Enter: o Claude Code lê ESC+CR como quebra de linha, como o Alt+Enter do Ghostty
        "enter" if m.control || m.shift || m.alt => Some(b"\x1b\r".to_vec()),
        "enter" => Some(b"\r".to_vec()),
        "backspace" => Some(if m.alt {
            b"\x1b\x7f".to_vec()
        } else {
            b"\x7f".to_vec()
        }),
        "tab" => Some(if m.shift {
            b"\x1b[Z".to_vec()
        } else {
            b"\t".to_vec()
        }),
        "escape" => Some(b"\x1b".to_vec()),
        "up" => arrow(b'A'),
        "down" => arrow(b'B'),
        "right" => arrow(b'C'),
        "left" => arrow(b'D'),
        "home" => Some(b"\x1b[H".to_vec()),
        "end" => Some(b"\x1b[F".to_vec()),
        "pageup" => Some(b"\x1b[5~".to_vec()),
        "pagedown" => Some(b"\x1b[6~".to_vec()),
        "delete" => Some(b"\x1b[3~".to_vec()),
        "insert" => Some(b"\x1b[2~".to_vec()),
        "space" if m.control => Some(vec![0]),
        _ => None,
    };
    if named.is_some() {
        return named;
    }
    if m.control && !m.alt {
        let c = k.key.chars().next().filter(|_| k.key.len() == 1)?;
        let byte = match c {
            'a'..='z' => c as u8 - b'a' + 1,
            '@' | '2' => 0,
            '[' => 0x1b,
            '\\' => 0x1c,
            ']' => 0x1d,
            '^' | '6' => 0x1e,
            '_' | '-' => 0x1f,
            _ => return None,
        };
        return Some(vec![byte]);
    }
    // texto comum (inclusive acentos de tecla morta) chega pelo input handler, não por aqui
    if !m.alt {
        return None;
    }
    let text = k
        .key_char
        .clone()
        .or_else(|| (k.key.chars().count() == 1).then(|| k.key.clone()))?;
    let mut bytes = vec![0x1b];
    bytes.extend_from_slice(text.as_bytes());
    Some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ks(s: &str) -> Keystroke {
        Keystroke::parse(s).unwrap()
    }

    #[test]
    fn mapeia_teclas() {
        assert_eq!(to_bytes(&ks("enter"), false), Some(b"\r".to_vec()));
        for k in ["ctrl-enter", "shift-enter", "alt-enter"] {
            assert_eq!(to_bytes(&ks(k), false), Some(b"\x1b\r".to_vec()), "{k}");
        }
        assert_eq!(to_bytes(&ks("ctrl-c"), false), Some(vec![3]));
        assert_eq!(to_bytes(&ks("ctrl-a"), false), Some(vec![1]));
        assert_eq!(to_bytes(&ks("up"), false), Some(b"\x1b[A".to_vec()));
        assert_eq!(to_bytes(&ks("up"), true), Some(b"\x1bOA".to_vec()));
        assert_eq!(to_bytes(&ks("shift-tab"), false), Some(b"\x1b[Z".to_vec()));
        assert_eq!(to_bytes(&ks("a"), false), None);
        assert_eq!(to_bytes(&ks("alt-b"), false), Some(b"\x1bb".to_vec()));
        assert_eq!(to_bytes(&ks("ctrl-shift-2"), false), Some(vec![0]));
    }

    #[test]
    fn atalhos_do_maestro() {
        assert!(is_app_shortcut(&ks("ctrl-shift-t")));
        assert!(is_app_shortcut(&ks("ctrl-shift-enter")));
        assert!(is_app_shortcut(&ks("ctrl-shift-left")));
        assert!(!is_app_shortcut(&ks("ctrl-t")));
        assert!(is_app_shortcut(&ks("ctrl-shift-1")));
        assert!(is_app_shortcut(&ks("ctrl-!")));
        assert_eq!(feature_digit(&ks("ctrl-!")), Some(1));
        assert_eq!(feature_digit(&ks("ctrl-shift-9")), Some(9));
        assert_eq!(feature_digit(&ks("ctrl-1")), None);
        assert!(is_app_shortcut(&ks("ctrl-shift-space")));
        assert!(!is_app_shortcut(&ks("ctrl-shift--")));
        assert!(!is_app_shortcut(&ks("ctrl-alt-shift-t")));
        assert!(is_app_shortcut(&ks("alt-n")));
        assert!(is_app_shortcut(&ks("alt-t")));
        assert!(is_app_shortcut(&ks("alt-6")));
        assert!(is_app_shortcut(&ks("alt-7")));
        assert!(is_app_shortcut(&ks("alt-]")));
        assert!(!is_app_shortcut(&ks("alt-b")));
    }
}
