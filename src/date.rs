//! Date arithmetic, billing cycles, and relative-day phrasing.
//!
//! Every function takes its dates as arguments. The SDK's `Input` carries
//! `text`, `context`, and `command`, with no timestamp, so `execute` has no
//! clock to read. `cargo test` runs this module on the host target.

use serde::{Deserialize, Serialize};

/// A calendar date, stored and rendered as `YYYY-MM-DD`.
///
/// The derived `Ord` compares year, then month, then day. That is chronological
/// order, so the field order has to stay as it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Date {
    pub y: i32,
    pub m: u32,
    pub d: u32,
}

/// Days in `month` of `year`, Gregorian.
pub fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap(year) => 29,
        2 => 28,
        _ => 0,
    }
}

pub fn is_leap(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

/// Days since 1970-01-01 (Howard Hinnant's `days_from_civil`).
fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y } as i64;
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let mp = ((m + 9) % 12) as i64; // March = 0
    let doy = (153 * mp + 2) / 5 + d as i64 - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146097 + doe - 719468
}

/// Inverse of [`days_from_civil`] (Hinnant's `civil_from_days`).
fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    ((y + if m <= 2 { 1 } else { 0 }) as i32, m, d)
}

impl Date {
    /// Parse `YYYY-MM-DD`, rejecting impossible dates like `2026-02-30`.
    pub fn parse(s: &str) -> Option<Date> {
        let s = s.trim();
        let mut parts = s.split('-');
        let y: i32 = parts.next()?.parse().ok()?;
        let m: u32 = parts.next()?.parse().ok()?;
        let d: u32 = parts.next()?.parse().ok()?;
        if parts.next().is_some() || !(1..=12).contains(&m) || !(1900..=2200).contains(&y) {
            return None;
        }
        if d == 0 || d > days_in_month(y, m) {
            return None;
        }
        Some(Date { y, m, d })
    }

    /// Extract the date part of an RFC3339 timestamp, e.g. the tile's `now`.
    pub fn parse_rfc3339(s: &str) -> Option<Date> {
        Date::parse(s.split('T').next()?)
    }

    pub fn epoch_day(&self) -> i64 {
        days_from_civil(self.y, self.m, self.d)
    }

    pub fn from_epoch_day(z: i64) -> Date {
        let (y, m, d) = civil_from_days(z);
        Date { y, m, d }
    }

    pub fn add_days(&self, n: i64) -> Date {
        Date::from_epoch_day(self.epoch_day() + n)
    }

    /// Add whole months. The day clamps to the end of the target month, so
    /// 2026-01-31 plus one month is 2026-02-28.
    pub fn add_months(&self, n: i64) -> Date {
        let total = self.y as i64 * 12 + (self.m as i64 - 1) + n;
        let y = total.div_euclid(12) as i32;
        let m = total.rem_euclid(12) as u32 + 1;
        let d = self.d.min(days_in_month(y, m));
        Date { y, m, d }
    }

    /// Signed day count from `self` to `other`: negative means `other` is past.
    pub fn days_until(&self, other: Date) -> i64 {
        other.epoch_day() - self.epoch_day()
    }
}

impl std::fmt::Display for Date {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:04}-{:02}-{:02}", self.y, self.m, self.d)
    }
}

/// A billing period. Two shapes cover the real cadences. Fixed day counts
/// handle weekly, fortnightly, and "every 45 days". Calendar months handle
/// monthly, quarterly, and yearly, where month lengths vary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Cycle {
    Days(u32),
    Months(u32),
}

/// Mean Gregorian month length, for normalising day-based cycles to $/month.
const DAYS_PER_MONTH: f64 = 30.436_875;

impl Cycle {
    pub const WEEKLY: Cycle = Cycle::Days(7);
    pub const MONTHLY: Cycle = Cycle::Months(1);
    pub const QUARTERLY: Cycle = Cycle::Months(3);
    pub const YEARLY: Cycle = Cycle::Months(12);

    /// Accepts the words people actually type, plus `30d` / `2w` / `6m` forms.
    pub fn parse(s: &str) -> Option<Cycle> {
        let t = s.trim().trim_start_matches('/').to_lowercase();
        let t = t.trim();
        match t {
            "weekly" | "week" | "wk" | "w" | "every week" => return Some(Cycle::Days(7)),
            "biweekly" | "fortnightly" | "fortnight" | "every 2 weeks" | "every two weeks" => {
                return Some(Cycle::Days(14));
            }
            "monthly" | "month" | "mo" | "m" | "every month" | "per month" => {
                return Some(Cycle::Months(1));
            }
            "quarterly" | "quarter" | "q" | "every quarter" => return Some(Cycle::Months(3)),
            "semiannual" | "semi-annual" | "biannual" | "half-yearly" | "halfyearly" => {
                return Some(Cycle::Months(6));
            }
            "yearly" | "year" | "yr" | "y" | "annual" | "annually" | "per year" => {
                return Some(Cycle::Months(12));
            }
            _ => {}
        }

        // "45 days", "45d", "2 weeks", "2w", "6 months", "6m"
        let digits: String = t.chars().take_while(|c| c.is_ascii_digit()).collect();
        if digits.is_empty() {
            return None;
        }
        let n: u32 = digits.parse().ok()?;
        if n == 0 {
            return None;
        }
        let unit = t[digits.len()..].trim();
        match unit {
            "d" | "day" | "days" => Some(Cycle::Days(n)),
            "w" | "wk" | "week" | "weeks" => Some(Cycle::Days(n * 7)),
            "m" | "mo" | "month" | "months" => Some(Cycle::Months(n)),
            "y" | "yr" | "year" | "years" => Some(Cycle::Months(n * 12)),
            _ => None,
        }
    }

    /// The next due date one period after `from`.
    pub fn advance(&self, from: Date) -> Date {
        match *self {
            Cycle::Days(n) => from.add_days(n as i64),
            Cycle::Months(n) => from.add_months(n as i64),
        }
    }

    /// Advance until the date is after `today`. A subscription left unrenewed
    /// for months catches up in one step. Returns the new date and the number
    /// of periods skipped.
    pub fn advance_past(&self, from: Date, today: Date) -> (Date, u32) {
        let mut date = self.advance(from);
        let mut periods = 1;
        // 600 periods is ~50 years of monthly billing: enough for any real
        // backlog, and a hard stop against a pathological stored date.
        while date <= today && periods < 600 {
            date = self.advance(date);
            periods += 1;
        }
        (date, periods)
    }

    /// Cost per month, for comparing cycles on one scale.
    pub fn monthly_factor(&self) -> f64 {
        match *self {
            Cycle::Days(n) => DAYS_PER_MONTH / n as f64,
            Cycle::Months(n) => 1.0 / n as f64,
        }
    }

    /// Google Calendar RRULE for a recurring renewal event.
    pub fn rrule(&self) -> String {
        match *self {
            Cycle::Days(7) => "RRULE:FREQ=WEEKLY".into(),
            Cycle::Days(n) if n % 7 == 0 => format!("RRULE:FREQ=WEEKLY;INTERVAL={}", n / 7),
            Cycle::Days(n) => format!("RRULE:FREQ=DAILY;INTERVAL={n}"),
            Cycle::Months(1) => "RRULE:FREQ=MONTHLY".into(),
            Cycle::Months(12) => "RRULE:FREQ=YEARLY".into(),
            Cycle::Months(n) if n % 12 == 0 => format!("RRULE:FREQ=YEARLY;INTERVAL={}", n / 12),
            Cycle::Months(n) => format!("RRULE:FREQ=MONTHLY;INTERVAL={n}"),
        }
    }
}

impl std::fmt::Display for Cycle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            Cycle::Days(7) => write!(f, "weekly"),
            Cycle::Days(14) => write!(f, "every 2 weeks"),
            Cycle::Days(n) if n % 7 == 0 => write!(f, "every {} weeks", n / 7),
            Cycle::Days(n) => write!(f, "every {n} days"),
            Cycle::Months(1) => write!(f, "monthly"),
            Cycle::Months(3) => write!(f, "quarterly"),
            Cycle::Months(6) => write!(f, "every 6 months"),
            Cycle::Months(12) => write!(f, "yearly"),
            Cycle::Months(n) => write!(f, "every {n} months"),
        }
    }
}

/// Phrasing for the digest. A passed deadline reads "due 3 days ago" and
/// keeps counting until the subscription is renewed.
pub fn phrase_due(days: i64) -> String {
    match days {
        d if d < -1 => format!("due {} days ago", -d),
        -1 => "due yesterday".to_string(),
        0 => "due today".to_string(),
        1 => "due tomorrow".to_string(),
        d => format!("due in {d} days"),
    }
}

/// The same timing without the "due" prefix. Used where the sentence already
/// names the date: "cancel by 2027-05-15 (in 239 days)".
pub fn phrase_span(days: i64) -> String {
    match days {
        d if d < -1 => format!("{} days ago", -d),
        -1 => "yesterday".to_string(),
        0 => "today".to_string(),
        1 => "tomorrow".to_string(),
        d => format!("in {d} days"),
    }
}

/// Compact phrasing for tile badges, which cap at 40 characters.
pub fn phrase_badge(days: i64) -> String {
    match days {
        d if d < 0 => format!("{}d overdue", -d),
        0 => "due today".to_string(),
        1 => "tomorrow".to_string(),
        d => format!("in {d}d"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> Date {
        Date::parse(s).expect("valid date")
    }

    #[test]
    fn epoch_round_trips_across_eras() {
        for s in ["1970-01-01", "2026-09-17", "2000-02-29", "2100-03-01", "1999-12-31"] {
            let date = d(s);
            assert_eq!(Date::from_epoch_day(date.epoch_day()), date, "{s}");
            assert_eq!(date.to_string(), s);
        }
        assert_eq!(d("1970-01-01").epoch_day(), 0);
        assert_eq!(d("2026-09-17").epoch_day(), 20713);
    }

    #[test]
    fn parse_rejects_impossible_dates() {
        assert!(Date::parse("2026-02-30").is_none());
        assert!(Date::parse("2026-13-01").is_none());
        assert!(Date::parse("2026-00-10").is_none());
        assert!(Date::parse("2026-01-00").is_none());
        assert!(Date::parse("2026-1-1").is_some(), "lenient about zero padding");
        assert!(Date::parse("not-a-date").is_none());
        assert!(Date::parse("2026-01-01-01").is_none());
        assert_eq!(Date::parse("2025-02-28"), Some(Date { y: 2025, m: 2, d: 28 }));
        assert!(Date::parse("2025-02-29").is_none(), "2025 is not a leap year");
        assert!(Date::parse("2024-02-29").is_some(), "2024 is");
    }

    #[test]
    fn parse_rfc3339_takes_the_date_part() {
        assert_eq!(Date::parse_rfc3339("2026-09-17T19:30:00.123456+00:00"), Some(d("2026-09-17")));
        assert_eq!(Date::parse_rfc3339("2026-09-17T00:00:00Z"), Some(d("2026-09-17")));
        assert_eq!(Date::parse_rfc3339("garbage"), None);
    }

    #[test]
    fn month_addition_clamps_to_month_end() {
        assert_eq!(d("2026-01-31").add_months(1), d("2026-02-28"));
        assert_eq!(d("2024-01-31").add_months(1), d("2024-02-29"), "leap year");
        assert_eq!(d("2026-01-31").add_months(3), d("2026-04-30"));
        assert_eq!(d("2026-03-31").add_months(1), d("2026-04-30"));
        assert_eq!(d("2026-12-15").add_months(1), d("2027-01-15"), "year rollover");
        assert_eq!(d("2026-01-15").add_months(-1), d("2025-12-15"), "backwards");
        assert_eq!(d("2026-02-28").add_months(12), d("2027-02-28"));
    }

    #[test]
    fn day_addition_crosses_boundaries() {
        assert_eq!(d("2026-12-31").add_days(1), d("2027-01-01"));
        assert_eq!(d("2024-02-28").add_days(1), d("2024-02-29"));
        assert_eq!(d("2026-01-01").add_days(-1), d("2025-12-31"));
    }

    #[test]
    fn days_until_is_signed() {
        assert_eq!(d("2026-09-17").days_until(d("2026-09-20")), 3);
        assert_eq!(d("2026-09-17").days_until(d("2026-09-14")), -3);
        assert_eq!(d("2026-09-17").days_until(d("2026-09-17")), 0);
        assert_eq!(d("2026-01-01").days_until(d("2027-01-01")), 365);
        assert_eq!(d("2024-01-01").days_until(d("2025-01-01")), 366);
    }

    #[test]
    fn dates_order_chronologically() {
        assert!(d("2026-09-14") < d("2026-09-17"));
        assert!(d("2025-12-31") < d("2026-01-01"));
        let mut all = [d("2026-10-01"), d("2026-09-14"), d("2027-01-01")];
        all.sort();
        assert_eq!(all[0], d("2026-09-14"));
        assert_eq!(all[2], d("2027-01-01"));
    }

    #[test]
    fn cycle_parses_words_and_shorthand() {
        assert_eq!(Cycle::parse("monthly"), Some(Cycle::Months(1)));
        assert_eq!(Cycle::parse("  MONTHLY "), Some(Cycle::Months(1)));
        assert_eq!(Cycle::parse("/mo"), Some(Cycle::Months(1)));
        assert_eq!(Cycle::parse("yearly"), Some(Cycle::Months(12)));
        assert_eq!(Cycle::parse("annual"), Some(Cycle::Months(12)));
        assert_eq!(Cycle::parse("quarterly"), Some(Cycle::Months(3)));
        assert_eq!(Cycle::parse("weekly"), Some(Cycle::Days(7)));
        assert_eq!(Cycle::parse("biweekly"), Some(Cycle::Days(14)));
        assert_eq!(Cycle::parse("45 days"), Some(Cycle::Days(45)));
        assert_eq!(Cycle::parse("45d"), Some(Cycle::Days(45)));
        assert_eq!(Cycle::parse("2w"), Some(Cycle::Days(14)));
        assert_eq!(Cycle::parse("6 months"), Some(Cycle::Months(6)));
        assert_eq!(Cycle::parse("2 years"), Some(Cycle::Months(24)));
        assert_eq!(Cycle::parse("0 days"), None);
        assert_eq!(Cycle::parse("every so often"), None);
        assert_eq!(Cycle::parse(""), None);
    }

    #[test]
    fn cycle_advance_respects_month_ends() {
        assert_eq!(Cycle::MONTHLY.advance(d("2026-01-31")), d("2026-02-28"));
        assert_eq!(Cycle::WEEKLY.advance(d("2026-09-17")), d("2026-09-24"));
        assert_eq!(Cycle::YEARLY.advance(d("2026-09-17")), d("2027-09-17"));
    }

    #[test]
    fn advance_past_catches_up_a_backlog() {
        // Due in January, renewed in September: one step lands in the future.
        let (next, periods) = Cycle::MONTHLY.advance_past(d("2026-01-15"), d("2026-09-17"));
        assert_eq!(next, d("2026-10-15"));
        assert_eq!(periods, 9);

        // Normal on-time renewal advances exactly one period.
        let (next, periods) = Cycle::MONTHLY.advance_past(d("2026-09-14"), d("2026-09-17"));
        assert_eq!(next, d("2026-10-14"));
        assert_eq!(periods, 1);

        // Renewing early (due date still ahead) must not skip a period.
        let (next, periods) = Cycle::MONTHLY.advance_past(d("2026-09-20"), d("2026-09-17"));
        assert_eq!(next, d("2026-10-20"));
        assert_eq!(periods, 1);
    }

    #[test]
    fn monthly_factor_normalises_cycles() {
        let close = |a: f64, b: f64| (a - b).abs() < 0.01;
        assert!(close(Cycle::MONTHLY.monthly_factor() * 20.0, 20.0));
        assert!(close(Cycle::YEARLY.monthly_factor() * 120.0, 10.0));
        assert!(close(Cycle::QUARTERLY.monthly_factor() * 30.0, 10.0));
        assert!(close(Cycle::WEEKLY.monthly_factor() * 5.0, 21.74));
        assert!(close(Cycle::Days(30).monthly_factor() * 30.0, 30.44));
    }

    #[test]
    fn rrule_matches_the_cycle() {
        assert_eq!(Cycle::MONTHLY.rrule(), "RRULE:FREQ=MONTHLY");
        assert_eq!(Cycle::YEARLY.rrule(), "RRULE:FREQ=YEARLY");
        assert_eq!(Cycle::QUARTERLY.rrule(), "RRULE:FREQ=MONTHLY;INTERVAL=3");
        assert_eq!(Cycle::WEEKLY.rrule(), "RRULE:FREQ=WEEKLY");
        assert_eq!(Cycle::Days(14).rrule(), "RRULE:FREQ=WEEKLY;INTERVAL=2");
        assert_eq!(Cycle::Days(45).rrule(), "RRULE:FREQ=DAILY;INTERVAL=45");
        assert_eq!(Cycle::Months(24).rrule(), "RRULE:FREQ=YEARLY;INTERVAL=2");
    }

    #[test]
    fn cycle_displays_in_plain_english() {
        assert_eq!(Cycle::MONTHLY.to_string(), "monthly");
        assert_eq!(Cycle::YEARLY.to_string(), "yearly");
        assert_eq!(Cycle::QUARTERLY.to_string(), "quarterly");
        assert_eq!(Cycle::Days(7).to_string(), "weekly");
        assert_eq!(Cycle::Days(14).to_string(), "every 2 weeks");
        assert_eq!(Cycle::Days(21).to_string(), "every 3 weeks");
        assert_eq!(Cycle::Days(45).to_string(), "every 45 days");
        assert_eq!(Cycle::Months(6).to_string(), "every 6 months");
        assert_eq!(Cycle::Months(4).to_string(), "every 4 months");
    }

    #[test]
    fn cycle_serialises_as_stable_json() {
        assert_eq!(serde_json::to_string(&Cycle::Months(1)).unwrap(), r#"{"months":1}"#);
        assert_eq!(serde_json::to_string(&Cycle::Days(7)).unwrap(), r#"{"days":7}"#);
        let back: Cycle = serde_json::from_str(r#"{"months":3}"#).unwrap();
        assert_eq!(back, Cycle::QUARTERLY);
    }

    #[test]
    fn overdue_phrasing_counts_up_from_the_deadline() {
        assert_eq!(phrase_due(-3), "due 3 days ago");
        assert_eq!(phrase_due(-1), "due yesterday");
        assert_eq!(phrase_due(0), "due today");
        assert_eq!(phrase_due(1), "due tomorrow");
        assert_eq!(phrase_due(12), "due in 12 days");

        assert_eq!(phrase_span(-3), "3 days ago");
        assert_eq!(phrase_span(0), "today");
        assert_eq!(phrase_span(239), "in 239 days");

        assert_eq!(phrase_badge(-3), "3d overdue");
        assert_eq!(phrase_badge(0), "due today");
        assert_eq!(phrase_badge(1), "tomorrow");
        assert_eq!(phrase_badge(12), "in 12d");
        // Badges are capped at 40 chars by the host validator.
        assert!(phrase_badge(-9999).len() <= 40);
    }
}
