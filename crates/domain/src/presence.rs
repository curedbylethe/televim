//! A peer's online status, as the protocol reports it.
//!
//! A plain value with no clock and no formatting. A timestamp is carried as the
//! raw Unix seconds the peer was last online; turning it into "5 min ago" is a
//! display concern, decided against the display's own clock.

/// How a peer's presence reads at the moment it was reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Presence {
    /// The peer is online now.
    Online,

    /// The peer was last online at `was_online`, a Unix timestamp in seconds.
    Offline {
        /// When the peer was last online.
        was_online: i32,
    },

    /// The peer was last seen recently, in the protocol's coarsest recent bucket.
    Recently,

    /// The peer was last seen within the last week, in the protocol's bucket.
    LastWeek,

    /// The peer was last seen within the last month, in the protocol's bucket.
    LastMonth,

    /// The peer's last-seen time is restricted or unknown, so nothing is
    /// shown for it.
    Hidden,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_variant_can_be_constructed_and_compared() {
        let variants = [
            Presence::Online,
            Presence::Offline {
                was_online: 1_700_000_000,
            },
            Presence::Recently,
            Presence::LastWeek,
            Presence::LastMonth,
            Presence::Hidden,
        ];

        for (index, variant) in variants.iter().enumerate() {
            for (other_index, other) in variants.iter().enumerate() {
                assert_eq!(variant == other, index == other_index, "{variant:?}");
            }
        }
    }

    #[test]
    fn an_offline_peer_carries_the_timestamp_it_was_last_seen_at() {
        assert_eq!(
            Presence::Offline { was_online: 42 },
            Presence::Offline { was_online: 42 }
        );
        assert_ne!(
            Presence::Offline { was_online: 42 },
            Presence::Offline { was_online: 43 }
        );
    }
}
