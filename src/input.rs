use crate::protocol::Event;
use gpui::Keystroke;

/// Physical/logical identities only; action bindings belong to PHP in BOTH versions.
pub fn normalize(stroke: &Keystroke) -> Option<Event> {
    if stroke.modifiers.control || stroke.modifiers.alt || stroke.modifiers.platform {
        return None;
    }
    let name = stroke.key.as_str();
    let function_key = (0..=20).any(|n| name == format!("f{n}"));
    // GPUI macOS maps native special keys before ordinary characters. Fn can produce
    // a standalone delete/home/pageup/F-key. Never degrade Fn+ordinary letters.
    let standalone = match name {
        "up" | "down" | "left" | "right" | "enter" | "space" | "escape" | "backspace"
        | "delete" | "insert" | "home" | "end" | "do" | "find" | "help" | "next" | "previous"
        | "select" => Some(name),
        "pageup" => Some("page_up"),
        "pagedown" => Some("page_down"),
        "tab" if stroke.modifiers.shift => Some("shift_tab"),
        "tab" => Some("tab"),
        _ if function_key => Some(name),
        _ => None,
    };
    let key = if let Some(key) = standalone {
        key.to_owned()
    } else if !stroke.modifiers.function
        && name.len() == 1
        && name.as_bytes()[0].is_ascii_alphabetic()
    {
        // Match key_char only for the same letter, preserving reported case/caps lock.
        if let Some(character) = &stroke.key_char
            && character.eq_ignore_ascii_case(name)
        {
            character.clone()
        } else if stroke.modifiers.shift {
            name.to_ascii_uppercase()
        } else {
            name.to_owned()
        }
    } else {
        return None;
    };
    Some(Event::Key { key })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn key(name: &str) -> Keystroke {
        Keystroke {
            key: name.into(),
            ..Default::default()
        }
    }
    fn event(name: &str) -> Option<Event> {
        Some(Event::Key { key: name.into() })
    }
    #[test]
    fn every_letter_and_standalone_identity() {
        for name in (b'a'..=b'z')
            .chain(b'A'..=b'Z')
            .map(|b| (b as char).to_string())
            .chain((0..=20).map(|n| format!("f{n}")))
            .chain(
                [
                    "up",
                    "down",
                    "left",
                    "right",
                    "enter",
                    "space",
                    "escape",
                    "tab",
                    "backspace",
                    "delete",
                    "insert",
                    "home",
                    "end",
                    "do",
                    "find",
                    "help",
                    "next",
                    "previous",
                    "select",
                ]
                .map(str::to_owned),
            )
        {
            assert_eq!(normalize(&key(&name)), event(&name));
        }
        assert_eq!(normalize(&key("pageup")), event("page_up"));
        assert_eq!(normalize(&key("pagedown")), event("page_down"));
    }
    #[test]
    fn native_gpui_shift_and_key_char_case() {
        for name in ["c", "m", "t", "q", "w", "a", "s", "d"] {
            let mut stroke = key(name);
            stroke.modifiers.shift = true;
            assert_eq!(normalize(&stroke), event(&name.to_uppercase()));
            stroke.key_char = Some(name.to_uppercase());
            assert_eq!(normalize(&stroke), event(&name.to_uppercase()));
            stroke.modifiers.shift = false;
            assert_eq!(normalize(&stroke), event(&name.to_uppercase()));
            stroke.modifiers.shift = true;
            stroke.key_char = Some(name.into());
            assert_eq!(normalize(&stroke), event(name)); // reported caps+shift case
        }
        let mut tab = key("tab");
        tab.key_char = Some("\t".into());
        tab.modifiers.shift = true;
        assert_eq!(normalize(&tab), event("shift_tab"));
    }
    #[test]
    fn modifiers_never_degrade_chords() {
        for name in ["w", "c", "x", "tab", "f5", "up"] {
            for modifier in 0..3 {
                let mut stroke = key(name);
                match modifier {
                    0 => stroke.modifiers.platform = true,
                    1 => stroke.modifiers.control = true,
                    _ => stroke.modifiers.alt = true,
                }
                assert_eq!(normalize(&stroke), None);
            }
        }
        let mut stroke = key("w");
        stroke.modifiers.function = true;
        assert_eq!(normalize(&stroke), None);
        for name in ["f0", "f5", "f20", "delete", "home", "pageup"] {
            stroke.key = name.into();
            assert!(normalize(&stroke).is_some());
        }
        for name in ["f21", "f01", "F5", "KeyC", "0", "é", "", "shift"] {
            assert_eq!(normalize(&key(name)), None);
        }
    }
}
