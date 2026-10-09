//! What a peer's presence reads as on screen.
//!
//! Pure: the caller passes `now`, as [`crate::date::day_label`] requires, so the
//! words are a function of the presence and the reader's clock and nothing else.

use domain::presence::Presence;

use crate::date::day_label;

/// The words a presence reads as, or `None` when there is nothing to say.
///
/// `Hidden` says nothing, because the peer has restricted what is shown and an
/// absent row is the design's answer to a restricted field. An offline peer
/// whose time is the "no time" sentinel says nothing for the same reason: a
/// "last seen" with no date would claim a time nobody reported.
#[must_use]
pub fn wording(presence: Presence, now: i64) -> Option<String> {
    match presence {
        Presence::Online => Some("online".to_owned()),
        Presence::Offline { was_online } => {
            day_label(i64::from(was_online), now, 0).map(|day| match day.as_ref() {
                "Today" => "last seen today".to_owned(),
                "Yesterday" => "last seen yesterday".to_owned(),
                _ => format!("last seen on {day}"),
            })
        }
        Presence::Recently => Some("last seen recently".to_owned()),
        Presence::LastWeek => Some("last seen within a week".to_owned()),
        Presence::LastMonth => Some("last seen within a month".to_owned()),
        Presence::Hidden => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-09-20 00:00 UTC, and the reader's clock 18 days later.
    const SEEN: i64 = 1_789_862_400;
    const NOW: i64 = SEEN + 18 * 86_400;

    #[test]
    fn each_restricted_bucket_reads_as_its_own_sentence() {
        assert_eq!(wording(Presence::Online, NOW).as_deref(), Some("online"));
        assert_eq!(
            wording(Presence::Recently, NOW).as_deref(),
            Some("last seen recently")
        );
        assert_eq!(
            wording(Presence::LastWeek, NOW).as_deref(),
            Some("last seen within a week")
        );
        assert_eq!(
            wording(Presence::LastMonth, NOW).as_deref(),
            Some("last seen within a month")
        );
    }

    #[test]
    fn an_offline_peer_reads_as_the_date_they_were_last_online() {
        let was_online = i32::try_from(SEEN).expect("the fixture fits a protocol timestamp");
        assert_eq!(
            wording(Presence::Offline { was_online }, NOW).as_deref(),
            Some("last seen on Sep 20, 2026")
        );
    }

    #[test]
    fn a_hidden_peer_and_an_offline_one_with_no_time_say_nothing() {
        assert_eq!(wording(Presence::Hidden, NOW), None);
        assert_eq!(
            wording(Presence::Offline { was_online: 0 }, NOW),
            None,
            "the sentinel is no time, not 1970"
        );
    }
}
