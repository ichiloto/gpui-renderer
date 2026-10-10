use crate::protocol::Event;
use gpui::Keystroke;

/// Most controls a `key_transitions` session tracks as held at once. Real
/// keyboard rollover is far lower; beyond it a key-down is event-only.
pub const MAX_HELD_CONTROLS: usize = 64;

/// GPUI's names for the special keys the protocol reports, in wire form.
/// Tab is one physical control; Shift only changes the identity it types.
fn special_key(name: &str) -> Option<&str> {
    let function_key = (0..=20).any(|n| name == format!("f{n}"));
    match name {
        "up" | "down" | "left" | "right" | "enter" | "space" | "escape" | "backspace"
        | "delete" | "insert" | "home" | "end" | "do" | "find" | "help" | "next" | "previous"
        | "select" | "tab" => Some(name),
        "pageup" => Some("page_up"),
        "pagedown" => Some("page_down"),
        _ if function_key => Some(name),
        _ => None,
    }
}

/// Physical/logical identities only; action bindings belong to PHP in BOTH versions.
pub fn normalize(stroke: &Keystroke) -> Option<Event> {
    if stroke.modifiers.control || stroke.modifiers.alt || stroke.modifiers.platform {
        return None;
    }
    let name = stroke.key.as_str();
    // GPUI macOS maps native special keys before ordinary characters. Fn can produce
    // a standalone delete/home/pageup/F-key. Never degrade Fn+ordinary letters.
    let key = if let Some(key) = special_key(name) {
        if key == "tab" && stroke.modifiers.shift {
            "shift_tab".to_owned()
        } else {
            key.to_owned()
        }
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
    Some(Event::key(key))
}

/// The physical key a keystroke belongs to, independent of the text it
/// types: case, Shift, Caps Lock and other modifiers never change it, so a
/// release always matches its press.
pub fn control(stroke: &Keystroke) -> Option<String> {
    let name = stroke.key.as_str();
    if let Some(key) = special_key(name) {
        Some(key.to_owned())
    } else if name.len() == 1 && name.as_bytes()[0].is_ascii_alphabetic() {
        Some(name.to_ascii_lowercase())
    } else {
        None
    }
}

/// Held controls of a `key_transitions` session. OS auto-repeat is told
/// apart by control identity, not GPUI's `is_held`, which macOS does not
/// report for keys routed through its text input system (the arrows).
#[derive(Debug, Default)]
pub struct HeldControls {
    held: Vec<String>,
}

impl HeldControls {
    /// A key-down as a transition key event. A control already held repeats;
    /// only the first key-down presses it. Platform shortcuts stay ignored.
    pub fn press(&mut self, stroke: &Keystroke) -> Option<Event> {
        let Some(Event::Key { key, .. }) = normalize(stroke) else {
            return None;
        };
        let control = control(stroke)?;
        let repeat = self.held.contains(&control) || self.held.len() >= MAX_HELD_CONTROLS;
        if !repeat {
            self.held.push(control.clone());
        }
        Some(Event::Key {
            key,
            control: Some(control),
            repeat: Some(repeat),
        })
    }

    /// A key-up releases its control whatever modifiers are now down.
    pub fn release(&mut self, stroke: &Keystroke) -> Option<Event> {
        let control = control(stroke)?;
        let index = self.held.iter().position(|held| *held == control)?;
        self.held.remove(index);
        Some(Event::KeyRelease { control })
    }

    /// Forget every held control; PHP releases them all.
    pub fn reset(&mut self) -> Event {
        self.held.clear();
        Event::InputReset
    }

    /// macOS withholds key-up events while Command is down, so engaging it
    /// ends the renderer's knowledge of what is held.
    pub fn change_modifiers(&mut self, modifiers: gpui::Modifiers) -> Option<Event> {
        (modifiers.platform && !self.held.is_empty()).then(|| self.reset())
    }
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
        Some(Event::key(name))
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
    fn transition(key: &str, control: &str, repeat: bool) -> Option<Event> {
        Some(Event::Key {
            key: key.into(),
            control: Some(control.into()),
            repeat: Some(repeat),
        })
    }
    fn release(control: &str) -> Option<Event> {
        Some(Event::KeyRelease {
            control: control.into(),
        })
    }
    #[test]
    fn os_repeats_are_not_presses_and_release_matches_the_control() {
        let mut held = HeldControls::default();
        let down = key("down");
        assert_eq!(held.press(&down), transition("down", "down", false));
        // GPUI reports arrows from macOS text input with is_held false; the
        // control identity alone decides that these are repeats.
        assert_eq!(held.press(&down), transition("down", "down", true));
        assert_eq!(held.press(&down), transition("down", "down", true));
        assert_eq!(
            held.press(&key("right")),
            transition("right", "right", false)
        );
        assert_eq!(held.release(&key("right")), release("right"));
        assert_eq!(held.release(&key("right")), None);
        assert_eq!(held.release(&down), release("down"));
        assert_eq!(held.press(&down), transition("down", "down", false));
    }
    #[test]
    fn modifier_changes_never_strand_a_held_key() {
        let mut held = HeldControls::default();
        let mut s = key("s");
        assert_eq!(held.press(&s), transition("s", "s", false));
        // Shift now types S, but it is the same key, still held.
        s.modifiers.shift = true;
        assert_eq!(held.press(&s), transition("S", "s", true));
        s.modifiers.shift = false;
        s.modifiers.control = true;
        // A chord is a platform shortcut, never a press, but its release still releases.
        assert_eq!(held.press(&s), None);
        assert_eq!(held.release(&s), release("s"));
        let mut tab = key("tab");
        tab.modifiers.shift = true;
        assert_eq!(held.press(&tab), transition("shift_tab", "tab", false));
        tab.modifiers.shift = false;
        assert_eq!(held.release(&tab), release("tab"));
        assert_eq!(
            held.press(&key("pageup")),
            transition("page_up", "page_up", false)
        );
        assert_eq!(held.press(&key("é")), None);
    }
    #[test]
    fn command_and_resets_release_everything_held() {
        let mut held = HeldControls::default();
        let command = gpui::Modifiers {
            platform: true,
            ..Default::default()
        };
        assert_eq!(held.change_modifiers(command), None);
        held.press(&key("left"));
        held.press(&key("w"));
        assert_eq!(held.change_modifiers(gpui::Modifiers::default()), None);
        let shift = gpui::Modifiers {
            shift: true,
            ..Default::default()
        };
        assert_eq!(held.change_modifiers(shift), None);
        assert_eq!(held.change_modifiers(command), Some(Event::InputReset));
        assert_eq!(held.release(&key("left")), None);
        assert_eq!(held.press(&key("left")), transition("left", "left", false));
        assert_eq!(held.reset(), Event::InputReset);
        assert_eq!(held.release(&key("left")), None);
    }
    #[test]
    fn held_controls_stay_bounded() {
        let mut held = HeldControls::default();
        let names: Vec<String> = (0..=20)
            .map(|n| format!("f{n}"))
            .chain((b'a'..=b'z').map(|b| (b as char).to_string()))
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
            .collect();
        assert!(names.len() > MAX_HELD_CONTROLS);
        let presses = names
            .iter()
            .filter(|name| {
                matches!(
                    held.press(&key(name)),
                    Some(Event::Key {
                        repeat: Some(false),
                        ..
                    })
                )
            })
            .count();
        assert_eq!(presses, MAX_HELD_CONTROLS);
        assert!(matches!(
            held.press(&key("select")),
            Some(Event::Key {
                repeat: Some(true),
                ..
            })
        ));
    }
    #[test]
    fn transition_events_serialize_only_when_negotiated_fields_exist() {
        let line = |event: &Event| {
            let mut bytes = Vec::new();
            crate::protocol::write_event(&mut bytes, crate::protocol::Version::V2, event).unwrap();
            String::from_utf8(bytes).unwrap()
        };
        assert_eq!(
            line(&Event::key("up")),
            "{\"protocol\":2,\"type\":\"key\",\"key\":\"up\"}\n"
        );
        assert_eq!(
            line(&transition("S", "s", true).unwrap()),
            "{\"protocol\":2,\"type\":\"key\",\"key\":\"S\",\"control\":\"s\",\"repeat\":true}\n"
        );
        assert_eq!(
            line(&release("s").unwrap()),
            "{\"protocol\":2,\"type\":\"key_release\",\"control\":\"s\"}\n"
        );
        assert_eq!(
            line(&Event::InputReset),
            "{\"protocol\":2,\"type\":\"input_reset\"}\n"
        );
    }
}
