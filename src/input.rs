use crate::protocol::Event;
use gpui::Keystroke;

/// Physical/logical identities only; action bindings belong to PHP.
pub fn normalize(stroke: &Keystroke) -> Option<Event> {
    // v1 has no modifier-chord vocabulary. Never turn cmd-W into plain W.
    if stroke.modifiers.control
        || stroke.modifiers.alt
        || stroke.modifiers.platform
        || stroke.modifiers.function
    {
        return None;
    }
    let key = match stroke.key.as_str() {
        "up" | "down" | "left" | "right" | "enter" | "space" | "escape" => stroke.key.clone(),
        "w" | "a" | "s" | "d" | "q" | "W" | "A" | "S" | "D" | "Q" => {
            // key_char preserves caps-lock/keyboard-layout case where available.
            if let Some(character) = &stroke.key_char {
                if character.eq_ignore_ascii_case(&stroke.key) {
                    character.clone()
                } else if stroke.modifiers.shift {
                    stroke.key.to_ascii_uppercase()
                } else {
                    stroke.key.clone()
                }
            } else if stroke.modifiers.shift {
                stroke.key.to_ascii_uppercase()
            } else {
                stroke.key.clone()
            }
        }
        _ => return None,
    };
    Some(Event::Key { key })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn required_keys_serialize_as_identities() {
        for key in [
            "up", "down", "left", "right", "w", "a", "s", "d", "W", "A", "S", "D", "enter",
            "space", "escape", "q", "Q",
        ] {
            let event = normalize(&Keystroke {
                key: key.into(),
                ..Default::default()
            })
            .unwrap();
            assert_eq!(event, Event::Key { key: key.into() });
            let mut bytes = Vec::new();
            crate::protocol::write_event(&mut bytes, &event).unwrap();
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()["key"],
                key
            );
        }
    }
    #[test]
    fn shift_caps_lock_and_unsupported_chords() {
        let mut stroke = Keystroke {
            key: "w".into(),
            ..Default::default()
        };
        stroke.modifiers.shift = true;
        assert_eq!(normalize(&stroke), Some(Event::Key { key: "W".into() }));
        stroke.modifiers.shift = false;
        stroke.key_char = Some("W".into());
        assert_eq!(normalize(&stroke), Some(Event::Key { key: "W".into() }));
        stroke.modifiers.platform = true;
        assert!(normalize(&stroke).is_none());
        assert!(
            normalize(&Keystroke {
                key: "f1".into(),
                ..Default::default()
            })
            .is_none()
        );
    }
}
