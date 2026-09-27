//! The `$ddev` sidebar token text.

use crate::busy::Verb;
use crate::docker::State;

pub const RUNNING: &str = "● ddev";
pub const PAUSED: &str = "◐ ddev";
pub const STOPPED: &str = "○ ddev";
pub const BUSY: &str = "◌ ddev…";

/// The `$ddev` token for a project: busy while an action runs, else its state.
pub fn text(state: State, busy: Option<Verb>) -> &'static str {
    match (busy, state) {
        (Some(_), _) => BUSY,
        (None, State::Running) => RUNNING,
        (None, State::Paused) => PAUSED,
        (None, State::Stopped) => STOPPED,
    }
}

/// Just the symbol, for picker rows.
pub fn symbol(state: State, busy: Option<Verb>) -> &'static str {
    text(state, busy).split(' ').next().unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_follows_state() {
        assert_eq!(text(State::Running, None), "● ddev");
        assert_eq!(text(State::Paused, None), "◐ ddev");
        assert_eq!(text(State::Stopped, None), "○ ddev");
    }

    #[test]
    fn busy_wins_over_state() {
        assert_eq!(text(State::Running, Some(Verb::Stop)), "◌ ddev…");
    }

    #[test]
    fn symbol_is_the_first_character() {
        assert_eq!(symbol(State::Running, None), "●");
        assert_eq!(symbol(State::Stopped, Some(Verb::Start)), "◌");
    }
}
