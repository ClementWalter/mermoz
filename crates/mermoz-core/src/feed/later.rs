//! Later: when a snoozed mail comes back. Quick options ("Friday evening") and a small
//! French/English parser for what the user types ("vendredi soir", "friday 7pm",
//! "dans 3 jours", "15 nov"), all local: no AI call, instant while typing.

use super::dates;
use chrono::{Datelike, Duration, NaiveDate, NaiveDateTime, NaiveTime, Timelike, Weekday};
use regex::Regex;
use serde::Serialize;
use std::sync::LazyLock;

/// The hours behind "morning", "noon", "afternoon", "evening" and "this weekend".
pub const MORNING: u32 = 8;
pub const NOON: u32 = 12;
pub const AFTERNOON: u32 = 14;
pub const EVENING: u32 = 18;
pub const WEEKEND_HOUR: u32 = 9;
/// "This evening" is offered until this hour.
pub const EVENING_OFFERED_BEFORE: u32 = 17;

/// A ready-made choice in the Later picker; `key` is its one-key shortcut there.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct LaterOption {
    pub key: String,
    pub label: String,
    /// When it comes back, local time (see [`millis`] for the protocol).
    #[serde(skip)]
    pub at: NaiveDateTime,
}

/// Unix milliseconds of a local date and time (the earlier one across a DST change).
pub fn millis(at: NaiveDateTime) -> Option<i64> {
    at.and_local_timezone(chrono::Local)
        .earliest()
        .map(|t| t.timestamp_millis())
}

/// Local now, for parsing and options.
pub fn now() -> NaiveDateTime {
    chrono::Local::now().naive_local()
}

fn at(day: NaiveDate, hour: u32) -> NaiveDateTime {
    day.and_time(NaiveTime::from_hms_opt(hour, 0, 0).unwrap())
}

/// The next such weekday strictly after `day`.
fn next(day: NaiveDate, w: Weekday) -> NaiveDate {
    dates::next_weekday(w, day)
}

/// The quick choices for `now`: this evening (until 17:00), tomorrow morning, this
/// weekend (on weekdays), Friday evening (Monday to Thursday), next week.
pub fn options(now: NaiveDateTime) -> Vec<LaterOption> {
    let today = now.date();
    let wd = today.weekday();
    let mut out = vec![];
    let mut push = |key: &str, label: &str, when: NaiveDateTime| {
        out.push(LaterOption {
            key: key.into(),
            label: label.into(),
            at: when,
        })
    };
    if now.hour() < EVENING_OFFERED_BEFORE {
        push("e", "This evening", at(today, EVENING));
    }
    push(
        "t",
        "Tomorrow morning",
        at(today + Duration::days(1), MORNING),
    );
    if !matches!(wd, Weekday::Sat | Weekday::Sun) {
        push(
            "w",
            "This weekend",
            at(next(today, Weekday::Sat), WEEKEND_HOUR),
        );
    }
    if matches!(
        wd,
        Weekday::Mon | Weekday::Tue | Weekday::Wed | Weekday::Thu
    ) {
        push(
            "f",
            "Friday evening",
            at(next(today, Weekday::Fri), EVENING),
        );
    }
    push("n", "Next week", at(next(today, Weekday::Mon), MORNING));
    out
}

const WEEKDAYS: &[(&str, Weekday)] = &[
    ("lundi", Weekday::Mon),
    ("monday", Weekday::Mon),
    ("mardi", Weekday::Tue),
    ("tuesday", Weekday::Tue),
    ("mercredi", Weekday::Wed),
    ("wednesday", Weekday::Wed),
    ("jeudi", Weekday::Thu),
    ("thursday", Weekday::Thu),
    ("vendredi", Weekday::Fri),
    ("friday", Weekday::Fri),
    ("samedi", Weekday::Sat),
    ("saturday", Weekday::Sat),
    ("dimanche", Weekday::Sun),
    ("sunday", Weekday::Sun),
    ("mon", Weekday::Mon),
    ("tue", Weekday::Tue),
    ("wed", Weekday::Wed),
    ("thu", Weekday::Thu),
    ("fri", Weekday::Fri),
    ("sat", Weekday::Sat),
    ("sun", Weekday::Sun),
];

static CLOCK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(?P<h>\d{1,2})(?:\s?h\s?(?P<hm>\d{2})?|:(?P<cm>\d{2})(?:\s?(?P<ap1>am|pm))?|\s?(?P<ap2>am|pm))\b").unwrap()
});
static IN_N: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(?:dans|in)\s+(?P<n>\d+)\s*(?P<u>jours?|j|days?|d|semaines?|weeks?|w|heures?|hours?|h|minutes?|mins?)\b")
        .unwrap()
});
static WORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[\p{L}']+").unwrap());

/// What part of the text set the day, to know how to roll a past result forward.
#[derive(Clone, Copy, PartialEq)]
enum DaySource {
    Weekday,
    Written,
    Relative,
}

/// The hour of day the text asks for, if any: a clock time or a word for a time of day.
fn time_of_day(text: &str) -> Option<(u32, u32)> {
    if let Some(c) = CLOCK.captures(text) {
        let mut h: u32 = c["h"].parse().ok()?;
        let m: u32 = c
            .name("hm")
            .or(c.name("cm"))
            .and_then(|m| m.as_str().parse().ok())
            .unwrap_or(0);
        match c.name("ap1").or(c.name("ap2")).map(|a| a.as_str()) {
            Some("pm") if h < 12 => h += 12,
            Some("am") if h == 12 => h = 0,
            _ => {}
        }
        if h < 24 && m < 60 {
            return Some((h, m));
        }
    }
    let has = |w: &[&str]| w.iter().any(|w| text.contains(w));
    if has(&["après-midi", "apres-midi", "aprem", "afternoon"]) {
        Some((AFTERNOON, 0))
    } else if has(&["soir", "evening", "tonight"]) {
        Some((EVENING, 0))
    } else if has(&["matin", "morning"]) {
        Some((MORNING, 0))
    } else if has(&["midi", "noon", "lunch"]) {
        Some((NOON, 0))
    } else {
        None
    }
}

/// When "Later" ends for what the user typed, relative to `now` (local time). `None`
/// when the text names no day and no time.
pub fn parse(text: &str, now: NaiveDateTime) -> Option<NaiveDateTime> {
    let t = text.trim().to_lowercase().replace('’', "'");
    if t.is_empty() {
        return None;
    }
    let today = now.date();
    if let Some(c) = IN_N.captures(&t) {
        let n: i64 = c["n"].parse().ok()?;
        let u = &c["u"];
        if u.starts_with('h') {
            return Some(now + Duration::hours(n));
        }
        if u.starts_with('m') {
            return Some(now + Duration::minutes(n));
        }
        let days = if u.starts_with('s') || u.starts_with('w') {
            7 * n
        } else {
            n
        };
        let (h, m) = time_of_day(&t).unwrap_or((MORNING, 0));
        return today
            .checked_add_signed(Duration::days(days))?
            .and_hms_opt(h, m, 0);
    }
    let words: Vec<&str> = WORD.find_iter(&t).map(|m| m.as_str()).collect();
    let said = |w: &str| words.contains(&w);
    let phrase = |p: &str| t.contains(p);
    let mut weekend = false;
    let day: Option<(NaiveDate, DaySource)> =
        if phrase("après-demain") || phrase("apres-demain") || phrase("day after tomorrow") {
            Some((today + Duration::days(2), DaySource::Relative))
        } else if said("demain") || said("tomorrow") {
            Some((today + Duration::days(1), DaySource::Relative))
        } else if said("aujourd'hui")
            || said("today")
            || said("tonight")
            || phrase("ce soir")
            || phrase("ce matin")
            || phrase("cet après-midi")
            || phrase("this evening")
            || phrase("this morning")
            || phrase("this afternoon")
        {
            Some((today, DaySource::Relative))
        } else if phrase("semaine prochaine") || phrase("next week") {
            Some((next(today, Weekday::Mon), DaySource::Relative))
        } else if phrase("week-end") || phrase("weekend") {
            weekend = true;
            let sat = if today.weekday() == Weekday::Sat {
                today
            } else {
                next(today, Weekday::Sat)
            };
            Some((sat, DaySource::Weekday))
        } else if let Some(w) = WEEKDAYS.iter().find(|(n, _)| said(n)).map(|(_, w)| *w) {
            let d = if today.weekday() == w {
                today
            } else {
                next(today, w)
            };
            // "next friday" on a Monday is still this Friday; on a Friday it is the next one.
            Some((d, DaySource::Weekday))
        } else {
            dates::candidates(&t, today)
                .into_iter()
                .find(|c| !WEEKDAYS.iter().any(|(n, _)| c.span.to_lowercase() == *n))
                .map(|c| (c.date, DaySource::Written))
        };
    let time = time_of_day(&t);
    let (day, source) = match (day, time) {
        (None, None) => return None,
        (None, Some(_)) => (today, DaySource::Relative),
        (Some(d), _) => d,
    };
    let default_hour = if weekend { WEEKEND_HOUR } else { MORNING };
    let (h, m) = time.unwrap_or((default_hour, 0));
    let mut when = day.and_hms_opt(h, m, 0)?;
    // A moment already past rolls forward to its next occurrence.
    if when <= now {
        when += match source {
            DaySource::Weekday => Duration::days(7),
            DaySource::Written => Duration::days(365),
            DaySource::Relative => Duration::days(1),
        };
    }
    Some(when)
}

/// How the picker and the done strip say when it comes back: "today, 18:00",
/// "tomorrow, 08:00", "Fri 18:00" within a week, else "Sat 15 Nov, 08:00".
pub fn describe(when: NaiveDateTime, now: NaiveDateTime) -> String {
    let days = (when.date() - now.date()).num_days();
    let clock = when.format("%H:%M");
    match days {
        0 => format!("today, {clock}"),
        1 => format!("tomorrow, {clock}"),
        2..=6 => format!("{} {clock}", when.format("%a")),
        _ => format!("{}, {clock}", when.format("%a %-d %b")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    /// Monday 12 October 2026 at 10:00.
    fn monday() -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 10, 12)
            .unwrap()
            .and_hms_opt(10, 0, 0)
            .unwrap()
    }

    /// `days` after Monday 12 October 2026, at `h:m`.
    fn on(days: i64, h: u32, m: u32) -> NaiveDateTime {
        (monday().date() + Duration::days(days))
            .and_hms_opt(h, m, 0)
            .unwrap()
    }

    #[rstest]
    #[case::vendredi_soir("vendredi soir", on(4, 18, 0))]
    #[case::friday_7pm("friday 7pm", on(4, 19, 0))]
    #[case::friday_19h30("vendredi 19h30", on(4, 19, 30))]
    #[case::lundi_matin_next_week("lundi matin", on(7, 8, 0))]
    #[case::next_friday("next friday", on(4, 8, 0))]
    #[case::demain("demain", on(1, 8, 0))]
    #[case::tomorrow_afternoon("tomorrow afternoon", on(1, 14, 0))]
    #[case::apres_demain_midi("après-demain midi", on(2, 12, 0))]
    #[case::ce_soir("ce soir", on(0, 18, 0))]
    #[case::tonight("tonight", on(0, 18, 0))]
    #[case::dans_3_jours("dans 3 jours", on(3, 8, 0))]
    #[case::in_2_weeks("in 2 weeks", on(14, 8, 0))]
    #[case::in_3_hours("in 3 hours", on(0, 13, 0))]
    #[case::next_week("next week", on(7, 8, 0))]
    #[case::semaine_prochaine("la semaine prochaine", on(7, 8, 0))]
    #[case::weekend("ce week-end", on(5, 9, 0))]
    #[case::written_date("15 nov", on(34, 8, 0))]
    #[case::written_date_and_time("15 novembre à 9h", on(34, 9, 0))]
    #[case::numeric_date("20/10", on(8, 8, 0))]
    #[case::clock_only_later_today("16h", on(0, 16, 0))]
    #[case::clock_already_past_is_tomorrow("9h", on(1, 9, 0))]
    #[case::morning_already_past_is_tomorrow("ce matin", on(1, 8, 0))]
    #[case::same_weekday_past_is_next_week("lundi 9h", on(7, 9, 0))]
    #[case::same_weekday_later_today("lundi soir", on(0, 18, 0))]
    #[case::past_written_date_is_next_year("1er octobre", on(354, 8, 0))]
    fn parses_when_to_come_back(#[case] text: &str, #[case] want: NaiveDateTime) {
        assert_eq!(parse(text, monday()), Some(want));
    }

    #[rstest]
    #[case::empty("")]
    #[case::gibberish("bientôt peut-être")]
    fn unclear_text_is_none(#[case] text: &str) {
        assert_eq!(parse(text, monday()), None);
    }

    fn keys(now: NaiveDateTime) -> Vec<String> {
        options(now).into_iter().map(|o| o.key).collect()
    }

    #[rstest]
    #[case::monday_morning(on(0, 10, 0), vec!["e", "t", "w", "f", "n"])]
    #[case::monday_late(on(0, 17, 30), vec!["t", "w", "f", "n"])]
    #[case::thursday(on(3, 10, 0), vec!["e", "t", "w", "f", "n"])]
    #[case::friday(on(4, 10, 0), vec!["e", "t", "w", "n"])]
    #[case::saturday(on(5, 10, 0), vec!["e", "t", "n"])]
    fn quick_options_follow_the_weekday(#[case] now: NaiveDateTime, #[case] want: Vec<&str>) {
        assert_eq!(keys(now), want);
    }

    #[rstest]
    fn friday_evening_is_this_friday_at_six() {
        let f = options(monday())
            .into_iter()
            .find(|o| o.key == "f")
            .unwrap();
        assert_eq!(f.at, on(4, EVENING, 0));
    }

    #[rstest]
    fn this_weekend_is_saturday_morning() {
        let w = options(monday())
            .into_iter()
            .find(|o| o.key == "w")
            .unwrap();
        assert_eq!(w.at, on(5, WEEKEND_HOUR, 0));
    }

    #[rstest]
    fn next_week_is_monday_morning_even_on_sunday() {
        let n = options(on(6, 10, 0))
            .into_iter()
            .find(|o| o.key == "n")
            .unwrap();
        assert_eq!(n.at, on(7, MORNING, 0));
    }

    #[rstest]
    #[case::today(on(0, 18, 0), "today, 18:00")]
    #[case::tomorrow(on(1, 8, 0), "tomorrow, 08:00")]
    #[case::this_week(on(4, 18, 0), "Fri 18:00")]
    #[case::further(on(34, 8, 0), "Sun 15 Nov, 08:00")]
    fn says_when_in_plain_words(#[case] when: NaiveDateTime, #[case] want: &str) {
        assert_eq!(describe(when, monday()), want);
    }
}
