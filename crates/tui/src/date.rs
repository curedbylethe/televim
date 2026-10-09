//! Civil-date arithmetic: day keys, weekday names, day labels and `HH:MM`.
//!
//! Every function here is a **pure function of its arguments**. Nothing in this
//! module reads a clock: the caller passes `now` (unix seconds), which is how
//! the rest of the workspace passes time too (`Instant` into `app`, `tokio`
//! owning the loop). A formatter that called `SystemTime::now()` could not be
//! tested at a fixed instant, and "Today" would be untestable by construction.
//!
//! ## Timestamps
//!
//! `Message::timestamp` (`domain::message::Message`) is unix **seconds**, taken
//! straight from Telegram with no transform (`telegram-framework/src/history.rs`).
//! Timestamps are converted by **floor division** into a day key, so a
//! timestamp before the epoch lands on the correct earlier day rather than
//! truncating toward the epoch.
//!
//! ## The "no time" sentinel
//!
//! A locally queued send is written with `timestamp: 0`
//! (`ConversationView::queue_send`) until the server assigns one, and `0` is
//! 1970-01-01. Anything `timestamp <= 0` therefore means **no time**, not
//! "the first day of the epoch": [`has_time`] reports it, and every function
//! that would otherwise put a day boundary or a label on screen returns
//! `None` instead of inventing a date. Grouping and separators inherit the
//! neighbouring message's day rather than starting a 1970 bucket.
//!
//! ## Timezone
//!
//! Every function that turns a timestamp into a day or an `HH:MM` takes the
//! local offset from UTC, in seconds, as its `offset` argument and adds it
//! before any arithmetic. [`day_key`] and [`clock`] apply it; [`civil_from_timestamp`],
//! [`weekday_name`] and [`day_label`] apply the same offset. An offset of `0` is
//! UTC. The sentinel check reads the raw timestamp, before the offset is added.
//! The caller supplies the offset: this module reads no zone. Why the reader's
//! zone is used, and why it is passed in rather than read here, is the entry
//! "Why message times and day breaks render in the reader's local zone" in
//! `docs/decisions.md`.

use std::borrow::Cow;

/// Seconds in a civil day.
const SECONDS_PER_DAY: i64 = 86_400;

/// The month names, January first.
const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// The weekday names, Thursday first: day key `0` is 1970-01-01, a Thursday.
const WEEKDAYS: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];

/// A date with no time and no zone, as `YYYY-MM-DD` counts it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CivilDate {
    pub year: i64,
    /// 1-12.
    pub month: u32,
    /// 1-31.
    pub day: u32,
}

/// Whether a timestamp carries a time at all.
///
/// The "no time" sentinel, asked directly, so a caller that only needs the
/// distinction never has to compare against `0` itself.
#[must_use]
pub fn has_time(timestamp: i64) -> bool {
    timestamp > 0
}

/// The local day a timestamp falls on at `offset` seconds from UTC: `0` is
/// 1970-01-01 at offset `0`, `20716` is 2026-09-20.
///
/// Two timestamps share a day key **iff** they fall on the same local day at
/// `offset`, which is what makes "a separator sits exactly between two days"
/// decidable.
///
/// `None` for the [`has_time`] sentinel, which has no day of its own. The
/// sentinel is checked on the raw timestamp, before `offset` is added.
#[must_use]
pub fn day_key(timestamp: i64, offset: i64) -> Option<i64> {
    if has_time(timestamp) {
        Some(timestamp.saturating_add(offset).div_euclid(SECONDS_PER_DAY))
    } else {
        None
    }
}

/// The civil date a day key names. The inverse of [`days_from_civil`].
#[must_use]
pub fn civil_from_days(days: i64) -> CivilDate {
    // Howard Hinnant's civil_from_days, shifted so that day 0 is the epoch.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { y + 1 } else { y };

    CivilDate {
        year,
        month: u32::try_from(month).expect("month of a civil date is 1-12"),
        day: u32::try_from(day).expect("day of a civil date is 1-31"),
    }
}

/// The day key a civil date names. The inverse of [`civil_from_days`].
///
/// Accepts an out-of-range `month`/`day` the way the arithmetic does — a
/// `month` of 13 rolls into the next year — because the caller already knows
/// what it holds.
#[must_use]
pub fn days_from_civil(date: CivilDate) -> i64 {
    // Howard Hinnant's days_from_civil. March is the first month of the year
    // here, so the leap day lands at the end and no month needs a length table.
    let m = i64::from(date.month);
    let d = i64::from(date.day);
    let y = if m <= 2 { date.year - 1 } else { date.year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;

    era * 146_097 + doe - 719_468
}

/// The civil date a timestamp falls on at `offset` seconds from UTC.
///
/// Undefined by contract for the [`has_time`] sentinel, which this reports as
/// the epoch rather than refusing: it is a conversion, not a label, and no
/// caller should be printing one for a message that has no time.
#[must_use]
pub fn civil_from_timestamp(timestamp: i64, offset: i64) -> CivilDate {
    civil_from_days(timestamp.saturating_add(offset).div_euclid(SECONDS_PER_DAY))
}

/// The weekday a timestamp falls on at `offset` seconds from UTC, `Mon`-style
/// and abbreviated.
///
/// Undefined by contract for the [`has_time`] sentinel, as
/// [`civil_from_timestamp`].
#[must_use]
pub fn weekday_name(timestamp: i64, offset: i64) -> &'static str {
    // 1970-01-01 is day key 0 and a Thursday, which is index 0 of WEEKDAYS.
    let index = timestamp
        .saturating_add(offset)
        .div_euclid(SECONDS_PER_DAY)
        .rem_euclid(7);
    usize::try_from(index).map_or("Thu", |i| WEEKDAYS[i])
}

/// The `HH:MM` a timestamp shows at `offset` seconds from UTC, zero-padded,
/// always 24-hour.
///
/// `None` for the [`has_time`] sentinel: a message with no time shows no time.
/// The sentinel is checked on the raw timestamp, before `offset` is added.
#[must_use]
pub fn clock(timestamp: i64, offset: i64) -> Option<String> {
    if !has_time(timestamp) {
        return None;
    }
    let seconds = timestamp.saturating_add(offset).rem_euclid(SECONDS_PER_DAY);
    let (hour, minute) = (seconds / 3_600, (seconds % 3_600) / 60);

    Some(format!("{hour:02}:{minute:02}"))
}

/// The label a day separator carries: `Today`, `Yesterday`, the weekday for the
/// five days before that, and `Sep 20, 2026` past the week.
///
/// `now` is passed in, never read: this is a function of the two timestamps and
/// `offset`, which both are mapped through as [`day_key`] does.
/// `None` for the [`has_time`] sentinel, and for a timestamp whose day is more
/// than six days after `now`'s — a clock-skewed message gets a plain date
/// rather than a weekday name that would place it in the future.
#[must_use]
pub fn day_label(timestamp: i64, now: i64, offset: i64) -> Option<Cow<'static, str>> {
    let day = day_key(timestamp, offset)?;
    let today = day_key(now, offset)?;

    match day - today {
        0 => Some(Cow::Borrowed("Today")),
        -1 => Some(Cow::Borrowed("Yesterday")),
        delta if (-6..=6).contains(&delta) => Some(Cow::Borrowed(weekday_name(timestamp, offset))),
        _ => {
            let date = civil_from_days(day);
            let month = MONTHS
                .get(usize::try_from(date.month).expect("month of a civil date is 1-12") - 1)
                .expect("month names cover 1-12");
            Some(Cow::Owned(format!("{month} {}, {}", date.day, date.year)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fixed `now`: Sunday 2026-09-20, 15:04 UTC.
    const NOW: i64 = 1_789_916_640;

    fn at(year: i64, month: u32, day: u32) -> i64 {
        days_from_civil(CivilDate { year, month, day }) * SECONDS_PER_DAY
    }

    // ---- the day key (AC-7) -----------------------------------------------

    #[test]
    fn day_key_of_the_epoch_is_zero() {
        assert_eq!(
            day_key(0, 0),
            None,
            "0 is the no-time sentinel, not 1970-01-01"
        );
        assert_eq!(day_key(1, 0), Some(0));
    }

    #[test]
    fn day_key_is_the_same_within_a_day() {
        assert_eq!(day_key(at(2026, 9, 20), 0), Some(20_716));
        assert_eq!(day_key(at(2026, 9, 20) + 3_599, 0), Some(20_716));
        assert_eq!(
            day_key(at(2026, 9, 20) + SECONDS_PER_DAY - 1, 0),
            Some(20_716)
        );
    }

    #[test]
    fn day_key_crosses_at_midnight() {
        let last_second = 1_789_862_399; // 2026-09-19 23:59:59 UTC
        let first_second = 1_789_862_400; // 2026-09-20 00:00:00 UTC
        assert_ne!(day_key(last_second, 0), day_key(first_second, 0));
        assert_eq!(day_key(last_second, 0), Some(20_715));
        assert_eq!(day_key(first_second, 0), Some(20_716));
    }

    #[test]
    fn day_key_floors_timestamps_before_the_epoch() {
        assert_eq!(day_key(-1, 0), None);
        assert_eq!(day_key(-1 - SECONDS_PER_DAY, 0), None);
        assert_eq!(civil_from_timestamp(-86_400 - 1, 0).year, 1969);
        assert_eq!(civil_from_timestamp(-86_400, 0).year, 1969);
        assert_eq!(civil_from_timestamp(-86_400, 0).month, 12);
        assert_eq!(civil_from_timestamp(-86_400, 0).day, 31);
    }

    // ---- the no-time sentinel (T3) ---------------------------------------

    #[test]
    fn no_time_sentinel_has_no_day_key_and_no_label_and_no_clock() {
        for timestamp in [0, -1, i64::MIN] {
            assert!(!has_time(timestamp), "timestamp {timestamp} is no time");
            assert_eq!(day_key(timestamp, 0), None);
            assert_eq!(day_label(timestamp, NOW, 0), None);
            assert_eq!(clock(timestamp, 0), None);
        }
    }

    #[test]
    fn a_real_timestamp_has_a_time() {
        assert!(has_time(NOW));
        assert!(has_time(1));
    }

    // ---- civil dates ------------------------------------------------------

    #[test]
    fn civil_from_the_epoch() {
        assert_eq!(
            civil_from_timestamp(0, 0),
            CivilDate {
                year: 1970,
                month: 1,
                day: 1
            }
        );
    }

    #[test]
    fn civil_keeps_the_leap_day() {
        assert_eq!(
            civil_from_timestamp(951_868_799, 0), // 2000-02-29 23:59:59 UTC
            CivilDate {
                year: 2000,
                month: 2,
                day: 29
            }
        );
        // 1900-03-01, because 1900 was not a leap year and the century rule is
        // the one most hand-written implementations get wrong. 1900-02-28 is the
        // day before it.
        let march_1900 = -2_203_891_200;
        assert_eq!(civil_from_timestamp(march_1900, 0).month, 3);
        assert_eq!(civil_from_timestamp(march_1900, 0).day, 1);
        let feb_1900 = civil_from_timestamp(march_1900 - SECONDS_PER_DAY, 0);
        assert_eq!((feb_1900.month, feb_1900.day), (2, 28));
    }

    #[test]
    fn civil_round_trips_across_a_leap_year_and_a_century() {
        for timestamp in [1, 86_400, 951_868_799, -2_203_891_200, 4_007_836_799] {
            let date = civil_from_timestamp(timestamp, 0);
            assert_eq!(
                days_from_civil(date) * SECONDS_PER_DAY,
                timestamp - timestamp.rem_euclid(SECONDS_PER_DAY),
                "round trip failed for {timestamp}"
            );
        }
    }

    #[test]
    fn weekday_names_start_from_a_thursday() {
        assert_eq!(weekday_name(0, 0), "Thu");
        assert_eq!(weekday_name(SECONDS_PER_DAY, 0), "Fri");
        assert_eq!(weekday_name(3 * SECONDS_PER_DAY, 0), "Sun");
        assert_eq!(weekday_name(4 * SECONDS_PER_DAY, 0), "Mon");
        assert_eq!(weekday_name(NOW, 0), "Sun");
        assert_eq!(weekday_name(1_789_862_399, 0), "Sat");
    }

    // ---- the clock string -------------------------------------------------

    #[test]
    fn clock_is_zero_padded_24_hour() {
        assert_eq!(clock(0, 0), None);
        assert_eq!(clock(NOW, 0).as_deref(), Some("15:04"));
        assert_eq!(clock(at(2026, 9, 20), 0).as_deref(), Some("00:00"));
        assert_eq!(
            clock(at(2026, 9, 20) + SECONDS_PER_DAY - 1, 0).as_deref(),
            Some("23:59")
        );
        assert_eq!(
            clock(at(2026, 9, 20) + 9 * 3_600 + 5 * 60, 0).as_deref(),
            Some("09:05")
        );
    }

    #[test]
    fn clock_is_five_columns_wide_for_every_hour() {
        for hour in 0..24 {
            let timestamp = at(2026, 9, 20) + hour * 3_600;
            assert_eq!(
                clock(timestamp, 0).map(|s| s.len()),
                Some(5),
                "hour {hour} is not five columns"
            );
        }
    }

    // ---- the label table (AC-8) -------------------------------------------

    #[test]
    fn label_today_and_yesterday() {
        assert_eq!(day_label(NOW, NOW, 0).as_deref(), Some("Today"));
        assert_eq!(
            day_label(at(2026, 9, 20), NOW, 0).as_deref(),
            Some("Today"),
            "any time on today's day is Today"
        );
        assert_eq!(
            day_label(at(2026, 9, 19) + 23 * 3_600, NOW, 0).as_deref(),
            Some("Yesterday"),
            "yesterday's last hour is still Yesterday"
        );
    }

    #[test]
    fn label_is_the_weekday_for_the_five_days_before_yesterday() {
        let expected = [
            (2026, 9, 18, "Fri"),
            (2026, 9, 17, "Thu"),
            (2026, 9, 16, "Wed"),
            (2026, 9, 15, "Tue"),
            (2026, 9, 14, "Mon"),
        ];
        for (year, month, day, label) in expected {
            assert_eq!(
                day_label(at(year, month, day), NOW, 0).as_deref(),
                Some(label),
                "{year}-{month}-{day}"
            );
        }
    }

    #[test]
    fn label_is_a_full_date_past_the_week() {
        assert_eq!(
            day_label(at(2026, 9, 13), NOW, 0).as_deref(),
            Some("Sep 13, 2026"),
            "a week back is past the week, weekday or not"
        );
        assert_eq!(
            day_label(at(2026, 9, 12), NOW, 0).as_deref(),
            Some("Sep 12, 2026")
        );
        assert_eq!(
            day_label(at(2025, 12, 1), NOW, 0).as_deref(),
            Some("Dec 1, 2025")
        );
        assert_eq!(
            day_label(at(2026, 1, 1), NOW, 0).as_deref(),
            Some("Jan 1, 2026")
        );
    }

    #[test]
    fn label_survives_a_clock_skewed_message() {
        assert_eq!(
            day_label(at(2026, 9, 27), NOW, 0).as_deref(),
            Some("Sep 27, 2026"),
            "a message from the future gets a date, not a weekday name"
        );
    }

    #[test]
    fn label_treats_the_sentinel_as_no_time() {
        assert_eq!(day_label(0, NOW, 0), None);
        assert_eq!(
            day_label(-1, 0, 0),
            None,
            "no now is no day to be relative to"
        );
    }

    // ---- nonzero offsets --------------------------------------------------

    /// India Standard Time: a half-hour zone.
    const IST: i64 = 5 * 3_600 + 30 * 60;
    /// Pacific Standard Time.
    const PST: i64 = -8 * 3_600;

    #[test]
    fn clock_applies_a_half_hour_offset() {
        assert_eq!(clock(NOW, IST).as_deref(), Some("20:34"));
        assert_eq!(clock(at(2026, 9, 20), IST).as_deref(), Some("05:30"));
    }

    #[test]
    fn clock_applies_a_negative_offset() {
        assert_eq!(clock(NOW, PST).as_deref(), Some("07:04"));
        assert_eq!(
            clock(at(2026, 9, 20) + 3 * 3_600, PST).as_deref(),
            Some("19:00")
        );
    }

    #[test]
    fn day_key_moves_the_boundary_to_local_midnight() {
        // 2026-09-19 18:30 UTC is local midnight of 2026-09-20 at +05:30.
        let local_midnight = at(2026, 9, 19) + 18 * 3_600 + 30 * 60;
        assert_eq!(day_key(local_midnight - 1, IST), Some(20_715));
        assert_eq!(day_key(local_midnight, IST), Some(20_716));
        // 2026-09-20 08:00 UTC is local midnight of 2026-09-20 at -08:00.
        let local_midnight = at(2026, 9, 20) + 8 * 3_600;
        assert_eq!(day_key(local_midnight - 1, PST), Some(20_715));
        assert_eq!(day_key(local_midnight, PST), Some(20_716));
    }

    #[test]
    fn a_local_day_ahead_of_the_utc_day_agrees_across_the_readers() {
        // 2026-09-19 20:00 UTC: still Saturday the 19th in UTC, already
        // Sunday the 20th at +05:30.
        let instant = at(2026, 9, 19) + 20 * 3_600;
        let utc = civil_from_timestamp(instant, 0);
        let local = civil_from_timestamp(instant, IST);
        assert_eq!((utc.month, utc.day), (9, 19));
        assert_eq!((local.month, local.day), (9, 20));
        assert_eq!(weekday_name(instant, 0), "Sat");
        assert_eq!(weekday_name(instant, IST), "Sun");
        assert_eq!(day_key(instant, IST), Some(days_from_civil(local)));
        assert_eq!(clock(instant, IST).as_deref(), Some("01:30"));
        assert_eq!(day_label(instant, NOW, IST).as_deref(), Some("Today"));
        assert_eq!(day_label(instant, NOW, 0).as_deref(), Some("Yesterday"));
    }

    #[test]
    fn a_local_day_behind_the_utc_day_agrees_across_the_readers() {
        // 2026-09-20 03:00 UTC is still Saturday the 19th at -08:00.
        let instant = at(2026, 9, 20) + 3 * 3_600;
        assert_eq!(civil_from_timestamp(instant, PST).day, 19);
        assert_eq!(weekday_name(instant, PST), "Sat");
        assert_eq!(weekday_name(instant, 0), "Sun");
        assert_eq!(clock(instant, PST).as_deref(), Some("19:00"));
        assert_eq!(day_key(instant, PST), Some(20_715));
        // `NOW` is 07:04 on the 20th at -08:00, so the 19th is Yesterday.
        assert_eq!(day_label(instant, NOW, PST).as_deref(), Some("Yesterday"));
    }

    #[test]
    fn the_no_time_sentinel_stays_none_under_an_offset() {
        for offset in [IST, PST, SECONDS_PER_DAY] {
            assert_eq!(day_key(0, offset), None, "offset {offset}");
            assert_eq!(day_key(-1, offset), None, "offset {offset}");
            assert_eq!(day_key(i64::MIN, offset), None, "offset {offset}");
            assert_eq!(clock(0, offset), None, "offset {offset}");
            assert_eq!(day_label(0, NOW, offset), None, "offset {offset}");
        }
    }
}
