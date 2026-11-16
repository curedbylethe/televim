//! The voice note being played, as the screen knows it.
//!
//! A plain record. The player lives in the app, which owns the audio device
//! and pushes what it reports here; `tui` only holds it for the conversation
//! on show.

use std::time::Duration;

/// A voice note being played in the open conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NowPlaying {
    /// The message whose note this is.
    pub message_id: i64,

    /// Time played so far; holds while paused.
    pub position: Duration,

    /// Length of the note, once the player knows it.
    pub total: Option<Duration>,

    pub paused: bool,
}

/// The open conversation's playing note, if there is one.
#[derive(Debug, Default)]
pub struct PlaybackState {
    current: Option<NowPlaying>,
}

impl PlaybackState {
    /// Records `now` as the playing note, replacing any earlier one.
    pub fn set(&mut self, now: NowPlaying) {
        self.current = Some(now);
    }

    /// Forgets the playing note.
    pub fn clear(&mut self) {
        self.current = None;
    }

    /// The playing note, if one is.
    #[must_use]
    pub fn current(&self) -> Option<&NowPlaying> {
        self.current.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(message_id: i64, secs: u64) -> NowPlaying {
        NowPlaying {
            message_id,
            position: Duration::from_secs(secs),
            total: Some(Duration::from_secs(10)),
            paused: false,
        }
    }

    #[test]
    fn set_replaces_and_clear_forgets() {
        let mut state = PlaybackState::default();
        assert_eq!(state.current(), None);

        state.set(note(4, 1));
        state.set(note(5, 2));
        assert_eq!(state.current(), Some(&note(5, 2)));

        state.clear();
        assert_eq!(state.current(), None);
    }
}
