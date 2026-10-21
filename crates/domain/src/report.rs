//! The reasons Telegram accepts for a report.
//!
//! The wire names are hyphenated and the prompts read as sentences, so the
//! vocabulary is one enum with two spellings: [`ReportReason::token`] is what a
//! reader types after `:report`, and [`ReportReason::label`] is what the
//! confirmation prints. Keeping both here means the TUI parser and the protocol
//! translation cannot drift apart.

/// Why an account is being reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportReason {
    Spam,
    Violence,
    Pornography,
    ChildAbuse,
    Other,
    Copyright,
    GeoIrrelevant,
    Fake,
    IllegalDrugs,
    PersonalDetails,
}

impl ReportReason {
    /// The token a reader types after `:report`.
    #[must_use]
    pub const fn token(self) -> &'static str {
        match self {
            Self::Spam => "spam",
            Self::Violence => "violence",
            Self::Pornography => "pornography",
            Self::ChildAbuse => "child-abuse",
            Self::Other => "other",
            Self::Copyright => "copyright",
            Self::GeoIrrelevant => "geo-irrelevant",
            Self::Fake => "fake",
            Self::IllegalDrugs => "illegal-drugs",
            Self::PersonalDetails => "personal-details",
        }
    }

    /// The spelling a sentence uses, without the wire's hyphens.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Spam => "spam",
            Self::Violence => "violence",
            Self::Pornography => "pornography",
            Self::ChildAbuse => "child abuse",
            Self::Other => "other",
            Self::Copyright => "copyright",
            Self::GeoIrrelevant => "geo-irrelevant",
            Self::Fake => "fake",
            Self::IllegalDrugs => "illegal drugs",
            Self::PersonalDetails => "personal details",
        }
    }

    /// Reads a command token, or reports that it is not one.
    #[must_use]
    pub fn from_token(token: &str) -> Option<Self> {
        match token.trim() {
            "spam" => Some(Self::Spam),
            "violence" => Some(Self::Violence),
            "pornography" => Some(Self::Pornography),
            "child-abuse" => Some(Self::ChildAbuse),
            "other" => Some(Self::Other),
            "copyright" => Some(Self::Copyright),
            "geo-irrelevant" => Some(Self::GeoIrrelevant),
            "fake" => Some(Self::Fake),
            "illegal-drugs" => Some(Self::IllegalDrugs),
            "personal-details" => Some(Self::PersonalDetails),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_token_round_trips() {
        for reason in [
            ReportReason::Spam,
            ReportReason::Violence,
            ReportReason::Pornography,
            ReportReason::ChildAbuse,
            ReportReason::Other,
            ReportReason::Copyright,
            ReportReason::GeoIrrelevant,
            ReportReason::Fake,
            ReportReason::IllegalDrugs,
            ReportReason::PersonalDetails,
        ] {
            assert_eq!(ReportReason::from_token(reason.token()), Some(reason));
        }
    }

    #[test]
    fn the_label_is_not_the_token_for_two_reasons() {
        assert_eq!(ReportReason::ChildAbuse.token(), "child-abuse");
        assert_eq!(ReportReason::ChildAbuse.label(), "child abuse");
        assert_eq!(ReportReason::PersonalDetails.label(), "personal details");
    }

    #[test]
    fn an_unknown_token_is_none() {
        assert_eq!(ReportReason::from_token("nonsense"), None);
    }
}
