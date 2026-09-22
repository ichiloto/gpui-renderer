//! OS activation only. It does not claim visibility, minimization or occlusion.
use crate::protocol::Event;

#[derive(Default)]
pub struct ActivationState {
    last: Option<bool>,
}

impl ActivationState {
    pub fn update(&mut self, active: bool) -> Option<Event> {
        if self.last == Some(active) {
            return None;
        }
        self.last = Some(active);
        Some(Event::WindowActivation { active })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn initial_state_and_changes_are_reported_once() {
        let mut state = ActivationState::default();
        assert_eq!(
            state.update(false),
            Some(Event::WindowActivation { active: false })
        );
        assert_eq!(state.update(false), None);
        assert_eq!(
            state.update(true),
            Some(Event::WindowActivation { active: true })
        );
        assert_eq!(state.update(true), None);
        let mut bytes = Vec::new();
        crate::protocol::write_event(
            &mut bytes,
            crate::protocol::Version::V2,
            &Event::WindowActivation { active: false },
        )
        .unwrap();
        assert_eq!(
            String::from_utf8(bytes).unwrap(),
            "{\"protocol\":2,\"type\":\"window_activation\",\"active\":false}\n"
        );
    }
}
