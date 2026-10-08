//! The media downloads the reader can see, and how each one stands.
//!
//! Kept beside the conversation rather than in `domain`: a download is the
//! state of an attempt, not a fact about the message, and it is forgotten when
//! the attempt settles. The message row reads it to draw a progress token or a
//! failure, and `Esc` reads it to know there is something to stop.

/// The most downloads kept at once.
///
/// The reader asks for one at a time in practice. The cap keeps the table small
/// under the memory budget, and when it is full the oldest entry goes first.
pub const DOWNLOAD_LIMIT: usize = 8;

/// Where one download stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Downloading, with the bytes collected so far and the size Telegram
    /// declared when it declared one.
    InFlight {
        /// Bytes collected so far.
        downloaded: usize,

        /// Declared size, when Telegram gave one.
        total: Option<usize>,
    },

    /// Failed, with the reason the status line gives for it.
    Failed(String),
}

/// The downloads kept, each keyed by its conversation and message.
#[derive(Debug, Default)]
pub struct Downloads {
    entries: Vec<(i64, i64, Outcome)>,
}

impl Downloads {
    /// Records that a download of `message_id` in `chat_id` has started.
    pub fn start(&mut self, chat_id: i64, message_id: i64) {
        self.insert(
            chat_id,
            message_id,
            Outcome::InFlight {
                downloaded: 0,
                total: None,
            },
        );
    }

    /// Records the bytes collected so far, for a download that is in flight.
    ///
    /// A progress event for a download no longer kept is dropped: it has
    /// already settled, or been pushed out by newer ones.
    pub fn progress(
        &mut self,
        chat_id: i64,
        message_id: i64,
        downloaded: usize,
        total: Option<usize>,
    ) {
        if let Some((_, _, outcome)) = self.find_mut(chat_id, message_id) {
            *outcome = Outcome::InFlight { downloaded, total };
        }
    }

    /// Records that a download failed, and why.
    pub fn fail(&mut self, chat_id: i64, message_id: i64, reason: String) {
        self.insert(chat_id, message_id, Outcome::Failed(reason));
    }

    /// Forgets a download, because it finished or was cancelled.
    pub fn forget(&mut self, chat_id: i64, message_id: i64) {
        self.entries
            .retain(|(chat, message, _)| *chat != chat_id || *message != message_id);
    }

    /// Where the download of `message_id` in `chat_id` stands, if it is kept.
    #[must_use]
    pub fn get(&self, chat_id: i64, message_id: i64) -> Option<&Outcome> {
        self.entries
            .iter()
            .find(|(chat, message, _)| *chat == chat_id && *message == message_id)
            .map(|(_, _, outcome)| outcome)
    }

    /// Whether the download of `message_id` in `chat_id` is still running.
    #[must_use]
    pub fn is_in_flight(&self, chat_id: i64, message_id: i64) -> bool {
        matches!(
            self.get(chat_id, message_id),
            Some(Outcome::InFlight { .. })
        )
    }

    /// Replaces any entry for the message, then keeps the new one, evicting the
    /// oldest when the table is full.
    fn insert(&mut self, chat_id: i64, message_id: i64, outcome: Outcome) {
        self.forget(chat_id, message_id);

        if self.entries.len() == DOWNLOAD_LIMIT {
            self.entries.remove(0);
        }

        self.entries.push((chat_id, message_id, outcome));
    }

    fn find_mut(&mut self, chat_id: i64, message_id: i64) -> Option<&mut (i64, i64, Outcome)> {
        self.entries
            .iter_mut()
            .find(|(chat, message, _)| *chat == chat_id && *message == message_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A started download is in flight at zero, and progress replaces that
    /// with what has landed, keeping the declared total.
    #[test]
    fn a_started_download_is_in_flight_and_progress_moves_it_on() {
        let mut downloads = Downloads::default();

        downloads.start(7, 9);
        assert_eq!(
            downloads.get(7, 9),
            Some(&Outcome::InFlight {
                downloaded: 0,
                total: None,
            })
        );

        downloads.progress(7, 9, 42, Some(100));
        assert!(downloads.is_in_flight(7, 9));
        assert_eq!(
            downloads.get(7, 9),
            Some(&Outcome::InFlight {
                downloaded: 42,
                total: Some(100),
            })
        );
    }

    /// Progress for a download that is not kept does nothing, so a late event
    /// cannot put back an entry that has settled.
    #[test]
    fn progress_for_an_unkept_download_creates_nothing() {
        let mut downloads = Downloads::default();

        downloads.progress(7, 9, 42, None);

        assert_eq!(downloads.get(7, 9), None);
    }

    /// A settled download leaves the table: forgetting it is what puts the
    /// plain token back, and a failure is kept only until the next start.
    #[test]
    fn forgetting_a_download_and_failing_one_are_kept_apart() {
        let mut downloads = Downloads::default();
        downloads.start(7, 9);
        downloads.forget(7, 9);
        assert_eq!(downloads.get(7, 9), None, "a cancelled download is gone");

        downloads.fail(7, 9, "no route".to_owned());
        assert_eq!(
            downloads.get(7, 9),
            Some(&Outcome::Failed("no route".to_owned()))
        );
        assert!(!downloads.is_in_flight(7, 9));

        downloads.start(7, 9);
        assert!(
            downloads.is_in_flight(7, 9),
            "starting again replaces the failure rather than adding a second entry"
        );
        assert_eq!(downloads.entries.len(), 1);
    }

    /// The table is bounded: once it is full the oldest entry goes, so a long
    /// session cannot grow it without limit.
    #[test]
    fn the_table_is_bounded_and_evicts_the_oldest() {
        let mut downloads = Downloads::default();

        for message in 0..=i64::try_from(DOWNLOAD_LIMIT).expect("a small limit") {
            downloads.start(7, message);
        }

        assert_eq!(downloads.entries.len(), DOWNLOAD_LIMIT);
        assert_eq!(downloads.get(7, 0), None, "the first one was evicted");
        let last = i64::try_from(DOWNLOAD_LIMIT).expect("a small limit");
        assert!(downloads.is_in_flight(7, last));
    }
}
