//! Finds the dates a thread mentions, in French or English, resolved against the day the
//! mail was written. Tuned to over-find: Jev then picks which one is the deadline, so the
//! date shown on a card is always one written in the mail, never one the model made up.

use chrono::{Datelike, Duration, NaiveDate, Weekday};
use regex::Regex;
use std::sync::LazyLock;

/// A date written in the mail, with the time next to it when there is one ("18h30" → "18:30").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DateCandidate {
    pub span: String,
    pub date: NaiveDate,
    pub time: Option<String>,
}

/// Enough for any real thread; more would only blur Jev's choice.
pub const MAX_CANDIDATES: usize = 8;

const MONTHS: &[(&str, u32)] = &[
    ("janvier", 1),
    ("january", 1),
    ("janv", 1),
    ("jan", 1),
    ("février", 2),
    ("fevrier", 2),
    ("february", 2),
    ("févr", 2),
    ("fevr", 2),
    ("fév", 2),
    ("feb", 2),
    ("mars", 3),
    ("march", 3),
    ("mar", 3),
    ("avril", 4),
    ("april", 4),
    ("avr", 4),
    ("apr", 4),
    ("mai", 5),
    ("may", 5),
    ("juin", 6),
    ("june", 6),
    ("jun", 6),
    ("juillet", 7),
    ("july", 7),
    ("juil", 7),
    ("jul", 7),
    ("août", 8),
    ("aout", 8),
    ("august", 8),
    ("aug", 8),
    ("septembre", 9),
    ("september", 9),
    ("sept", 9),
    ("sep", 9),
    ("octobre", 10),
    ("october", 10),
    ("oct", 10),
    ("novembre", 11),
    ("november", 11),
    ("nov", 11),
    ("décembre", 12),
    ("decembre", 12),
    ("december", 12),
    ("déc", 12),
    ("dec", 12),
];

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
];

const RELATIVE: &[(&str, i64)] = &[
    ("après-demain", 2),
    ("apres-demain", 2),
    ("day after tomorrow", 2),
    ("aujourd'hui", 0),
    ("aujourd’hui", 0),
    ("ce soir", 0),
    ("tonight", 0),
    ("today", 0),
    ("demain", 1),
    ("tomorrow", 1),
];

fn alternation(words: impl Iterator<Item = &'static str>) -> String {
    // Longest first, so "septembre" is not cut to "sept".
    let mut w: Vec<&str> = words.collect();
    w.sort_by_key(|s| std::cmp::Reverse(s.chars().count()));
    w.iter()
        .map(|s| regex::escape(s))
        .collect::<Vec<_>>()
        .join("|")
}

static MONTH: LazyLock<String> = LazyLock::new(|| alternation(MONTHS.iter().map(|m| m.0)));
static WEEKDAY: LazyLock<String> = LazyLock::new(|| alternation(WEEKDAYS.iter().map(|w| w.0)));

/// A time right after a date: "à 18h", "18h30", "at 6pm", "6:00 pm", "18:00".
const TIME: &str = r"(?:\s*(?:,|à|a|at|@|vers|from|de)?\s*(?P<h>\d{1,2})(?:(?:h|:)(?P<m>\d{2})?\s?(?P<ap>am|pm)?|\s?(?P<ap2>am|pm))\b)?";

static DAY_MONTH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"(?i)\b(?:(?:{wd})\.?,?\s+)?(?P<d>\d{{1,2}})(?:er|st|nd|rd|th)?\s+(?P<mo>{mo})\.?(?:\s+(?P<y>\d{{4}}))?\b{TIME}",
        wd = *WEEKDAY,
        mo = *MONTH
    ))
    .unwrap()
});

static MONTH_DAY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"(?i)\b(?:(?:{wd})\.?,?\s+)?(?P<mo>{mo})\.?\s+(?P<d>\d{{1,2}})(?:st|nd|rd|th)?\b(?:,?\s+(?P<y>\d{{4}}))?{TIME}",
        wd = *WEEKDAY,
        mo = *MONTH
    ))
    .unwrap()
});

static NUMERIC: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"(?i)\b(?P<d>\d{{1,2}})(?:/(?P<mo>\d{{1,2}})(?:/(?P<y>\d{{4}}|\d{{2}}))?|\.(?P<mo2>\d{{1,2}})\.(?P<y2>\d{{4}}))\b{TIME}"
    ))
    .unwrap()
});

static ISO: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"\b(?P<y>\d{{4}})-(?P<mo>\d{{2}})-(?P<d>\d{{2}})\b{TIME}"
    ))
    .unwrap()
});

static LONE_WEEKDAY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"(?i)\b(?P<w>{wd})\b(?:\s+(?:prochain|next))?{TIME}",
        wd = *WEEKDAY
    ))
    .unwrap()
});

static RELATIVE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"(?i)(?:^|[^\p{{L}}])(?P<r>{})(?:[^\p{{L}}]|$){TIME}",
        alternation(RELATIVE.iter().map(|r| r.0))
    ))
    .unwrap()
});

fn month_number(word: &str) -> Option<u32> {
    let w = word.to_lowercase();
    let w = w.trim_end_matches('.');
    MONTHS.iter().find(|(n, _)| *n == w).map(|(_, m)| *m)
}

fn weekday(word: &str) -> Option<Weekday> {
    let w = word.to_lowercase();
    WEEKDAYS.iter().find(|(n, _)| *n == w).map(|(_, d)| *d)
}

/// "18", "30", "pm" → "18:30"; hours outside a day are not a time.
fn time_of(c: &regex::Captures) -> Option<String> {
    let mut h: u32 = c.name("h")?.as_str().parse().ok()?;
    let m: u32 = c
        .name("m")
        .map(|m| m.as_str().parse().unwrap_or(0))
        .unwrap_or(0);
    match c
        .name("ap")
        .or(c.name("ap2"))
        .map(|a| a.as_str().to_lowercase())
    {
        Some(ap) if ap == "pm" && h < 12 => h += 12,
        Some(ap) if ap == "am" && h == 12 => h = 0,
        _ => {}
    }
    (h < 24 && m < 60).then(|| format!("{h:02}:{m:02}"))
}

/// Without a year, the date closest after a month before the mail: "3 janvier" written
/// in December is next January.
fn with_year(day: u32, month: u32, year: Option<i32>, written: NaiveDate) -> Option<NaiveDate> {
    if let Some(y) = year {
        return NaiveDate::from_ymd_opt(y, month, day);
    }
    let floor = written - Duration::days(31);
    [written.year(), written.year() + 1]
        .into_iter()
        .filter_map(|y| NaiveDate::from_ymd_opt(y, month, day))
        .find(|d| *d >= floor)
}

fn year_of(c: &regex::Captures) -> Option<i32> {
    let y: i32 = c.name("y")?.as_str().parse().ok()?;
    Some(if y < 100 { 2000 + y } else { y })
}

/// The next such weekday strictly after the day the mail was written ("by Monday"
/// written on a Monday means the Monday after).
pub fn next_weekday(day: Weekday, written: NaiveDate) -> NaiveDate {
    let ahead = (7 + day.num_days_from_monday() as i64
        - written.weekday().num_days_from_monday() as i64)
        % 7;
    written + Duration::days(if ahead == 0 { 7 } else { ahead })
}

/// Every date the text mentions, in order of appearance, one entry per day.
pub fn candidates(text: &str, written: NaiveDate) -> Vec<DateCandidate> {
    let mut found: Vec<(usize, DateCandidate)> = vec![];
    let mut taken: Vec<std::ops::Range<usize>> = vec![];
    let mut push = |range: std::ops::Range<usize>, span: &str, date: Option<NaiveDate>, time| {
        if let Some(date) = date
            && !taken
                .iter()
                .any(|r| r.start < range.end && range.start < r.end)
        {
            taken.push(range.clone());
            found.push((
                range.start,
                DateCandidate {
                    span: span.trim().to_string(),
                    date,
                    time,
                },
            ));
        }
    };
    // Most specific forms first, so "jeudi 16 octobre" is not read as a lone "jeudi".
    for c in ISO.captures_iter(text) {
        let m = c.get(0).unwrap();
        let date = (|| {
            NaiveDate::from_ymd_opt(
                c["y"].parse().ok()?,
                c["mo"].parse().ok()?,
                c["d"].parse().ok()?,
            )
        })();
        push(m.range(), m.as_str(), date, time_of(&c));
    }
    for re in [&*DAY_MONTH, &*MONTH_DAY] {
        for c in re.captures_iter(text) {
            let m = c.get(0).unwrap();
            let date = (|| {
                with_year(
                    c["d"].parse().ok()?,
                    month_number(&c["mo"])?,
                    year_of(&c),
                    written,
                )
            })();
            push(m.range(), m.as_str(), date, time_of(&c));
        }
    }
    for c in NUMERIC.captures_iter(text) {
        let m = c.get(0).unwrap();
        // Dotted dates need their year: "2.4" is a version, not a date.
        let date = (|| {
            let mo = c.name("mo").or(c.name("mo2"))?.as_str().parse().ok()?;
            let y = year_of(&c).or_else(|| c.name("y2")?.as_str().parse().ok());
            with_year(c["d"].parse().ok()?, mo, y, written)
        })();
        push(m.range(), m.as_str(), date, time_of(&c));
    }
    for c in RELATIVE_RE.captures_iter(text) {
        let r = c.name("r").unwrap();
        let whole = c.get(0).unwrap();
        let days = RELATIVE
            .iter()
            .find(|(w, _)| w.eq_ignore_ascii_case(r.as_str()) || *w == r.as_str().to_lowercase())
            .map(|(_, d)| *d);
        let span = &text[r.start()..whole.end()];
        push(
            r.start()..whole.end(),
            span,
            days.map(|d| written + Duration::days(d)),
            time_of(&c),
        );
    }
    for c in LONE_WEEKDAY.captures_iter(text) {
        let m = c.get(0).unwrap();
        let date = weekday(&c["w"]).map(|d| next_weekday(d, written));
        push(m.range(), m.as_str(), date, time_of(&c));
    }
    found.sort_by_key(|(at, _)| *at);
    let mut out: Vec<DateCandidate> = vec![];
    for (_, c) in found {
        if !out.iter().any(|o| o.date == c.date) {
            out.push(c);
        }
        if out.len() == MAX_CANDIDATES {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    /// Monday 12 October 2026.
    fn monday() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, 12).unwrap()
    }

    fn first(text: &str) -> Option<String> {
        candidates(text, monday())
            .first()
            .map(|c| c.date.to_string())
    }

    #[rstest]
    #[case::french_day_month("Réponse avant le 16 octobre stp", "2026-10-16")]
    #[case::french_first("rendez-vous le 1er novembre", "2026-11-01")]
    #[case::french_weekday_day_month("on se voit vendredi 16 octobre", "2026-10-16")]
    #[case::dotted_with_year("le 15.10.2026", "2026-10-15")]
    #[case::english_month_day("the offsite on Oct 21", "2026-10-21")]
    #[case::english_ordinal("by October 21st, 2026", "2026-10-21")]
    #[case::english_day_month("due 20 Oct", "2026-10-20")]
    #[case::numeric("à régler avant le 20/10", "2026-10-20")]
    #[case::numeric_with_year("signé le 15/10/2026", "2026-10-15")]
    #[case::iso("deadline 2026-10-30", "2026-10-30")]
    #[case::tomorrow("can we talk tomorrow?", "2026-10-13")]
    #[case::demain("je passe demain", "2026-10-13")]
    #[case::after_tomorrow("après-demain ça marche", "2026-10-14")]
    #[case::lone_weekday("I need it by Friday", "2026-10-16")]
    #[case::same_weekday_is_next_week("lundi au plus tard", "2026-10-19")]
    #[case::next_year("le 3 janvier", "2027-01-03")]
    fn finds_the_date(#[case] text: &str, #[case] want: &str) {
        assert_eq!(first(text).as_deref(), Some(want));
    }

    #[rstest]
    #[case::plain("Merci pour ton retour, à bientôt")]
    #[case::version_number("see version 2.4 of the doc")]
    #[case::impossible_day("le 31/02")]
    fn finds_no_date(#[case] text: &str) {
        assert_eq!(first(text), None);
    }

    #[rstest]
    #[case::french_hours("mardi 14 octobre à 18h", Some("18:00"))]
    #[case::french_hours_minutes("le 14 octobre à 18h30", Some("18:30"))]
    #[case::english_pm("Oct 14 at 6pm", Some("18:00"))]
    #[case::english_colon("October 14, 6:30 pm", Some("18:30"))]
    #[case::no_time("le 14 octobre", None)]
    fn reads_the_time_next_to_the_date(#[case] text: &str, #[case] want: Option<&str>) {
        let c = candidates(text, monday());
        assert_eq!(c[0].time.as_deref(), want);
    }

    #[rstest]
    fn keeps_the_written_span() {
        assert_eq!(
            candidates("Réponse vendredi 16 octobre stp", monday())[0].span,
            "vendredi 16 octobre"
        );
    }

    #[rstest]
    fn one_entry_per_day() {
        assert_eq!(
            candidates("vendredi 16 octobre, donc vendredi", monday()).len(),
            1
        );
    }

    #[rstest]
    fn keeps_order_of_appearance() {
        let c = candidates("le 20/10 puis le 16 octobre", monday());
        assert_eq!(c[1].date.to_string(), "2026-10-16");
    }

    #[rstest]
    fn caps_the_number_of_candidates() {
        let text = (1..=20)
            .map(|d| format!("le {d}/11"))
            .collect::<Vec<_>>()
            .join(", ");
        assert_eq!(candidates(&text, monday()).len(), MAX_CANDIDATES);
    }

    #[rstest]
    #[case::later_this_week(Weekday::Thu, "2026-10-15")]
    #[case::same_day_means_next_week(Weekday::Mon, "2026-10-19")]
    #[case::sunday(Weekday::Sun, "2026-10-18")]
    fn next_weekday_is_strictly_after(#[case] day: Weekday, #[case] want: &str) {
        assert_eq!(next_weekday(day, monday()).to_string(), want);
    }
}
