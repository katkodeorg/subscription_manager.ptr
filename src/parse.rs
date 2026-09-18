//! Parsing of the typed command surface.
//!
//! Two kinds of input arrive here. A command the user or a tile button typed
//! is parsed by hand. A sentence from the AI orchestrator goes to the model and
//! comes back as JSON, parsed by [`AddSpec::from_json`]. Every function is
//! pure and unit-tested.

use crate::date::{Cycle, Date};

/// Flag keywords that end the `<name> <price> <cycle>` head of an add command.
const FLAGS: &[&str] = &[
    "next", "due", "on", "cat", "category", "card", "url", "cancel", "link", "trial", "note",
    "notes", "remind",
];

fn is_flag(token: &str) -> bool {
    FLAGS.contains(&token.trim().to_lowercase().as_str())
}

/// Parse an amount, tolerating currency symbols and thousands separators:
/// `$19.99`, `19,99`? no, `1,299.00`, `£5`, `9.99usd`.
pub fn parse_money(token: &str) -> Option<f64> {
    let token = token.trim();
    // A hyphen means this is a date (`2026-10-03`) or a range, never a price.
    // Without this guard the digit filter below would read that date as
    // 20261003, and `sub add ... next 2026-10-03` would price the entry at
    // twenty million.
    if token.contains('-') || token.contains(':') {
        return None;
    }
    let cleaned: String = token
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    if cleaned.is_empty() || cleaned.matches('.').count() > 1 {
        return None;
    }
    let value: f64 = cleaned.parse().ok()?;
    if !value.is_finite() {
        return None;
    }
    Some(value)
}

/// A date the user typed: an ISO date, `today`/`tomorrow`, or `+Nd`.
pub fn parse_when(value: &str, today: Date) -> Option<Date> {
    let v = value.trim().to_lowercase();
    match v.as_str() {
        "today" => return Some(today),
        "tomorrow" => return Some(today.add_days(1)),
        "yesterday" => return Some(today.add_days(-1)),
        _ => {}
    }
    if let Some(rest) = v.strip_prefix('+') {
        let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        if !digits.is_empty() {
            let n: i64 = digits.parse().ok()?;
            let unit = rest[digits.len()..].trim();
            return match unit {
                "" | "d" | "day" | "days" => Some(today.add_days(n)),
                "w" | "week" | "weeks" => Some(today.add_days(n * 7)),
                "m" | "month" | "months" => Some(today.add_months(n)),
                _ => None,
            };
        }
    }
    // "in 5 days" - the orchestrator phrases things this way.
    if let Some(rest) = v.strip_prefix("in ") {
        return parse_when(&format!("+{}", rest.trim()), today);
    }
    Date::parse(&v).or_else(|| parse_written_date(&v, today))
}

/// Month number for an English month name or its three-letter abbreviation.
fn month_number(word: &str) -> Option<u32> {
    const MONTHS: [&str; 12] = [
        "january", "february", "march", "april", "may", "june",
        "july", "august", "september", "october", "november", "december",
    ];
    let w = word.trim().trim_end_matches(&[',', '.'][..]).to_lowercase();
    if w.len() < 3 {
        return None;
    }
    MONTHS
        .iter()
        .position(|m| *m == w || m.starts_with(&w) && w.len() >= 3)
        .map(|i| i as u32 + 1)
}

/// Dates as people write them: `May 15 2027`, `15 May 2027`, `May 15, 2027`.
///
/// Every token has to be the month, the day, or the year. A caller uses this
/// to test whether a run of words is a date. "19.99 monthly may 15 2027" fails
/// and "may 15 2027" matches, which leaves the price and cycle alone.
fn parse_written_date(value: &str, today: Date) -> Option<Date> {
    let tokens: Vec<String> = value
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|t| !t.is_empty())
        .map(|t| t.trim_end_matches(&[',', '.'][..]).to_string())
        .collect();
    if tokens.len() < 2 || tokens.len() > 4 {
        return None;
    }

    let mut month: Option<u32> = None;
    let mut day: Option<u32> = None;
    let mut year: Option<i32> = None;

    for token in &tokens {
        if month.is_none()
            && let Some(m) = month_number(token) {
                month = Some(m);
                continue;
            }
        // Ordinal suffixes: 1st, 22nd, 3rd, 15th.
        let bare = token
            .trim_end_matches("st")
            .trim_end_matches("nd")
            .trim_end_matches("rd")
            .trim_end_matches("th");
        let Ok(n) = bare.parse::<i64>() else {
            return None;
        };
        if bare.len() == 4 && (1900..=2200).contains(&n) && year.is_none() {
            year = Some(n as i32);
        } else if (1..=31).contains(&n) && day.is_none() {
            day = Some(n as u32);
        } else {
            return None;
        }
    }

    let month = month?;
    let day = day.unwrap_or(1);
    // Without a year, take the next time that date comes around.
    let year = year.unwrap_or_else(|| {
        let clamped = day.min(crate::date::days_in_month(today.y, month));
        let candidate = Date { y: today.y, m: month, d: clamped };
        if candidate < today { today.y + 1 } else { today.y }
    });
    let day = day.min(crate::date::days_in_month(year, month));
    Date::parse(&format!("{year:04}-{month:02}-{day:02}"))
}

/// A subscription described by input, before it is given an id.
#[derive(Debug, Clone, PartialEq)]
pub struct AddSpec {
    pub name: String,
    /// `None` when the input never said what it costs.
    pub price: Option<f64>,
    pub cycle: Cycle,
    pub next_due: Option<Date>,
    pub category: Option<String>,
    pub card: Option<String>,
    pub cancel_url: Option<String>,
    pub trial_ends: Option<Date>,
    pub notes: Option<String>,
    pub reminder_days: Option<u32>,
    /// The line said this is a trial, so a bare date is its cancel-by date.
    pub is_trial: bool,
}

impl AddSpec {
    fn new(name: String, price: Option<f64>, cycle: Cycle) -> Self {
        AddSpec {
            name,
            price,
            cycle,
            next_due: None,
            category: None,
            card: None,
            cancel_url: None,
            trial_ends: None,
            notes: None,
            reminder_days: None,
            is_trial: false,
        }
    }

    /// Parse the JSON the AI extractor is asked to return. Tolerates code
    /// fences and surrounding prose, since models add both.
    pub fn from_json(raw: &str, today: Date) -> Result<AddSpec, String> {
        let slice = json_object_slice(raw).ok_or("No JSON object in the model's reply.")?;
        let v: serde_json::Value =
            serde_json::from_str(slice).map_err(|e| format!("Model returned invalid JSON: {e}"))?;

        let name = v["name"]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or("Could not find a subscription name.")?
            .to_string();

        // A price the text never mentioned stays unknown.
        let price = match &v["price"] {
            serde_json::Value::Number(n) => n.as_f64(),
            serde_json::Value::String(s) => parse_money(s),
            _ => None,
        };

        let cycle = v["cycle"]
            .as_str()
            .and_then(Cycle::parse)
            .unwrap_or(Cycle::MONTHLY);

        let mut spec = AddSpec::new(name, price, cycle);
        spec.next_due = v["next_due"].as_str().and_then(|s| parse_when(s, today));
        spec.trial_ends = v["trial_ends"].as_str().and_then(|s| parse_when(s, today));
        spec.is_trial = spec.trial_ends.is_some();
        spec.category = v["category"].as_str().map(clean_value).filter(|s| !s.is_empty());
        spec.cancel_url = v["cancel_url"].as_str().map(clean_value).filter(|s| !s.is_empty());
        spec.card = v["card"].as_str().map(clean_value).filter(|s| !s.is_empty());
        Ok(spec)
    }
}

fn clean_value(s: &str) -> String {
    s.trim().trim_matches('"').to_string()
}

/// Find the outermost `{...}` in a model reply.
fn json_object_slice(raw: &str) -> Option<&str> {
    let start = raw.find('{')?;
    let end = raw.rfind('}')?;
    if end > start { Some(&raw[start..=end]) } else { None }
}

/// What a free-form entry line yielded, before it is decided whether the date
/// is a renewal date or a trial's cancel-by date.
struct Entry {
    spec: AddSpec,
    date: Option<Date>,
}

/// Pull a name, an optional price, an optional cycle, and an optional date out
/// of one line, in any order.
///
/// The date is taken first. It is either a trailing run of tokens that reads as
/// a date ("may 15 2027") or a single ISO token anywhere in the line. What
/// remains is scanned for money, then for a cycle word. The rest is the name.
fn parse_entry(args: &str, today: Date) -> Result<Entry, String> {
    // "$19.99/mo" is one token to a human and two to the parser.
    let normalised = args.replace('/', " /");
    let tokens: Vec<&str> = normalised.split_whitespace().collect();
    if tokens.is_empty() {
        return Err("Usage: sub add <name> <price> <cycle>  e.g. `sub add Netflix 19.99 monthly`"
            .to_string());
    }

    let head_end = tokens.iter().position(|t| is_flag(t)).unwrap_or(tokens.len());
    let mut head: Vec<&str> = tokens[..head_end].to_vec();
    let tail = &tokens[head_end..];

    // A trailing date run, longest first, so "may 15 2027" wins over "2027".
    let mut date = None;
    for split in 1..head.len() {
        if let Some(found) = parse_when(&head[split..].join(" "), today) {
            date = Some(found);
            head.truncate(split);
            break;
        }
    }
    // Otherwise a single ISO date sitting anywhere in the line.
    if date.is_none()
        && let Some(idx) = head
            .iter()
            .position(|t| t.contains('-') && Date::parse(t).is_some())
        {
            date = Date::parse(head[idx]);
            head.remove(idx);
        }

    // The price is the last money-looking token; scanning from the right keeps
    // names with digits ("Adobe CC 2026") intact when a price follows them.
    let price_idx = head
        .iter()
        .rposition(|t| parse_money(t).is_some() && !t.starts_with('/'));

    let (name, cycle_text) = match price_idx {
        Some(0) => {
            return Err(
                "I need a name before the price, e.g. `sub add Netflix 19.99 monthly`.".to_string(),
            );
        }
        Some(idx) => (head[..idx].join(" "), head[idx + 1..].join(" ")),
        // No price: peel a trailing cycle off the name if there is one, so
        // `sub add AMC yearly` keeps the name as "AMC".
        None => {
            let split = (1..head.len())
                .rev()
                .find(|i| Cycle::parse(&head[*i..].join(" ")).is_some())
                .unwrap_or(head.len());
            (head[..split].join(" "), head[split..].join(" "))
        }
    };

    let name = name.trim().to_string();
    if name.is_empty() {
        return Err("I need a name, e.g. `sub add Netflix 19.99 monthly`.".to_string());
    }
    let price = price_idx.and_then(|idx| parse_money(head[idx]));

    let cycle = if cycle_text.trim().is_empty() {
        Cycle::MONTHLY
    } else {
        Cycle::parse(&cycle_text)
            .ok_or_else(|| format!("I don't understand the cycle \"{}\". Try monthly, yearly, weekly, quarterly, or `45 days`.", cycle_text.trim()))?
    };

    let mut spec = AddSpec::new(name, price, cycle);
    for (flag, value) in flag_pairs(tail) {
        apply_flag(&mut spec, &flag, &value, today)?;
    }
    Ok(Entry { spec, date })
}

/// Parse `<name...> [price] [cycle] [date] [flag value]...`.
///
/// A bare date is the end date: the day the next charge lands, or the day a
/// trial has to be cancelled by when the line says it is a trial.
pub fn parse_add(args: &str, today: Date) -> Result<AddSpec, String> {
    let Entry { mut spec, date } = parse_entry(args, today)?;
    if let Some(date) = date {
        // An explicit `trial <date>` flag already claimed the trial slot, so a
        // second bare date is the renewal date.
        // `add_from_spec` decides whether an unpriced entry is a trial. This
        // only routes the date.
        if spec.is_trial && spec.trial_ends.is_none() {
            spec.trial_ends = Some(date);
        } else {
            spec.next_due = Some(date);
        }
    }
    Ok(spec)
}

/// Parse `sub trial <name> <cancel-by date> [flags]`. The date is required,
/// since cancelling in time is the whole point.
pub fn parse_trial(args: &str, today: Date) -> Result<AddSpec, String> {
    let Entry { mut spec, date } = parse_entry(args, today)?;
    match date.or(spec.trial_ends) {
        Some(date) => {
            spec.trial_ends = Some(date);
            spec.is_trial = true;
            Ok(spec)
        }
        None => Err(
            "I need a cancel-by date, e.g. `sub trial AMC may 15 2027` or `sub trial AMC +14d`."
                .to_string(),
        ),
    }
}

/// Group `flag value value ... flag value` into pairs, joining multi-word values.
fn flag_pairs(tokens: &[&str]) -> Vec<(String, String)> {
    let mut pairs: Vec<(String, String)> = Vec::new();
    let mut current: Option<(String, Vec<String>)> = None;
    for token in tokens {
        if is_flag(token) {
            if let Some((flag, words)) = current.take() {
                pairs.push((flag, words.join(" ")));
            }
            current = Some((token.to_lowercase(), Vec::new()));
        } else if let Some((_, words)) = current.as_mut() {
            // "/" was inserted by normalisation; put it back for URL values.
            words.push(token.to_string());
        }
    }
    if let Some((flag, words)) = current {
        pairs.push((flag, words.join(" ")));
    }
    pairs
}

fn apply_flag(spec: &mut AddSpec, flag: &str, value: &str, today: Date) -> Result<(), String> {
    let value = value.trim();
    // `trial` also works as a bare marker. In `sub add AMC 2027-05-15 trial`
    // it makes the date a cancel-by date.
    if flag == "trial" && matches!(value.to_lowercase().as_str(), "" | "yes" | "true") {
        spec.is_trial = true;
        return Ok(());
    }
    if value.is_empty() {
        return Err(format!("`{flag}` needs a value."));
    }
    match flag {
        "next" | "due" | "on" => {
            spec.next_due = Some(parse_when(value, today).ok_or_else(|| {
                format!("I don't understand the date \"{value}\". Use YYYY-MM-DD, `tomorrow`, or `+10d`.")
            })?);
        }
        "trial" => {
            spec.trial_ends = Some(parse_when(value, today).ok_or_else(|| {
                format!("I don't understand the trial end date \"{value}\". Use YYYY-MM-DD or `+14d`.")
            })?);
            spec.is_trial = true;
        }
        "cat" | "category" => spec.category = Some(value.to_string()),
        "card" => spec.card = Some(value.trim_start_matches('*').to_string()),
        "url" | "cancel" | "link" => spec.cancel_url = Some(rejoin_url(value)),
        "note" | "notes" => spec.notes = Some(value.to_string()),
        "remind" => {
            let digits: String = value.chars().take_while(|c| c.is_ascii_digit()).collect();
            spec.reminder_days = Some(
                digits
                    .parse()
                    .map_err(|_| format!("`remind` needs a number of days, got \"{value}\"."))?,
            );
        }
        other => return Err(format!("Unknown option `{other}`.")),
    }
    Ok(())
}

/// Undo the `/` spacing that [`parse_add`] introduces, so URLs survive.
fn rejoin_url(value: &str) -> String {
    value.replace(" /", "/").replace("/ ", "/").trim().to_string()
}

/// Split `<target> <field> <value...>` for `sub edit`.
pub fn parse_edit(args: &str) -> Result<(String, String, String), String> {
    let tokens: Vec<&str> = args.split_whitespace().collect();
    // Find the first token that names an editable field; everything before it
    // is the subscription name, everything after is the value.
    let field_idx = tokens
        .iter()
        .position(|t| {
            matches!(
                t.to_lowercase().as_str(),
                "price" | "cost" | "cycle" | "next" | "due" | "cat" | "category" | "card"
                    | "url" | "cancel" | "trial" | "note" | "notes" | "name" | "remind"
                    | "cal" | "calendar"
            )
        })
        .ok_or_else(|| {
            "Usage: sub edit <name> <field> <value>. Fields: price, cycle, next, cat, card, url, trial, note, name, remind, cal."
                .to_string()
        })?;
    if field_idx == 0 {
        return Err("Name the subscription first, e.g. `sub edit netflix price 21.99`.".to_string());
    }
    let target = tokens[..field_idx].join(" ");
    let field = tokens[field_idx].to_lowercase();
    let value = tokens[field_idx + 1..].join(" ");
    if value.trim().is_empty() {
        return Err(format!("`{field}` needs a new value."));
    }
    Ok((target, field, value))
}

/// Split a command into its verb and the rest, after the `sub` prefix is gone.
pub fn split_verb(rest: &str) -> (String, &str) {
    let rest = rest.trim();
    match rest.split_once(char::is_whitespace) {
        Some((verb, args)) => (verb.to_lowercase(), args.trim()),
        None => (rest.to_lowercase(), ""),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn today() -> Date {
        Date::parse("2026-09-17").unwrap()
    }

    #[test]
    fn money_tolerates_symbols_and_separators() {
        assert_eq!(parse_money("19.99"), Some(19.99));
        assert_eq!(parse_money("$19.99"), Some(19.99));
        assert_eq!(parse_money("£5"), Some(5.0));
        assert_eq!(parse_money("1,299.00"), Some(1299.0));
        assert_eq!(parse_money("9.99usd"), Some(9.99));
        assert_eq!(parse_money("free"), None);
        assert_eq!(parse_money(""), None);
        assert_eq!(parse_money("2026-10-03"), None, "dates are not prices");
    }

    #[test]
    fn add_parses_the_common_case() {
        let spec = parse_add("Netflix 19.99 monthly", today()).unwrap();
        assert_eq!(spec.name, "Netflix");
        assert_eq!(spec.price, Some(19.99));
        assert_eq!(spec.cycle, Cycle::MONTHLY);
        assert_eq!(spec.next_due, None, "caller defaults this");
    }

    #[test]
    fn add_keeps_multi_word_names() {
        let spec = parse_add("Amazon Prime 14.99 yearly", today()).unwrap();
        assert_eq!(spec.name, "Amazon Prime");
        assert_eq!(spec.cycle, Cycle::YEARLY);

        let spec = parse_add("Adobe CC 2026 59.99 monthly", today()).unwrap();
        assert_eq!(spec.name, "Adobe CC 2026", "digits in the name survive");
        assert_eq!(spec.price, Some(59.99));
    }

    #[test]
    fn add_handles_slash_shorthand() {
        let spec = parse_add("Netflix $19.99/mo", today()).unwrap();
        assert_eq!(spec.name, "Netflix");
        assert_eq!(spec.price, Some(19.99));
        assert_eq!(spec.cycle, Cycle::MONTHLY);

        let spec = parse_add("Figma 144/yr", today()).unwrap();
        assert_eq!(spec.price, Some(144.0));
        assert_eq!(spec.cycle, Cycle::YEARLY);
    }

    #[test]
    fn add_defaults_the_cycle_to_monthly() {
        let spec = parse_add("Netflix 19.99", today()).unwrap();
        assert_eq!(spec.cycle, Cycle::MONTHLY);
    }

    #[test]
    fn add_parses_every_flag() {
        let spec = parse_add(
            "Netflix 19.99 monthly next 2026-10-03 cat streaming card 4242 \
             url https://netflix.com/cancelplan trial 2026-10-01 remind 5 note joint account",
            today(),
        )
        .unwrap();
        assert_eq!(spec.next_due, Date::parse("2026-10-03"));
        assert_eq!(spec.category.as_deref(), Some("streaming"));
        assert_eq!(spec.card.as_deref(), Some("4242"));
        assert_eq!(
            spec.cancel_url.as_deref(),
            Some("https://netflix.com/cancelplan"),
            "the / normalisation must not mangle URLs"
        );
        assert_eq!(spec.trial_ends, Date::parse("2026-10-01"));
        assert_eq!(spec.reminder_days, Some(5));
        assert_eq!(spec.notes.as_deref(), Some("joint account"));
    }

    #[test]
    fn add_accepts_relative_dates() {
        let spec = parse_add("Hulu 9.99 monthly next tomorrow", today()).unwrap();
        assert_eq!(spec.next_due, Date::parse("2026-09-18"));

        let spec = parse_add("Hulu 9.99 monthly due +10d", today()).unwrap();
        assert_eq!(spec.next_due, Date::parse("2026-09-27"));

        let spec = parse_add("Hulu 9.99 monthly on 2026-12-01", today()).unwrap();
        assert_eq!(spec.next_due, Date::parse("2026-12-01"));
    }

    #[test]
    fn add_rejects_input_it_cannot_understand() {
        assert!(parse_add("", today()).is_err());
        assert!(parse_add("19.99 ", today()).is_err(), "price with no name");
        assert!(parse_add("19.99", today()).is_err(), "no name");
        let err = parse_add("Netflix 19.99 whenever", today()).unwrap_err();
        assert!(err.contains("cycle"), "{err}");
        let err = parse_add("Netflix 19.99 monthly next notadate", today()).unwrap_err();
        assert!(err.contains("date"), "{err}");
    }

    #[test]
    fn relative_when_forms() {
        assert_eq!(parse_when("today", today()), Some(today()));
        assert_eq!(parse_when("tomorrow", today()), Date::parse("2026-09-18"));
        assert_eq!(parse_when("yesterday", today()), Date::parse("2026-09-16"));
        assert_eq!(parse_when("+3", today()), Date::parse("2026-09-20"));
        assert_eq!(parse_when("+2w", today()), Date::parse("2026-10-01"));
        assert_eq!(parse_when("+1m", today()), Date::parse("2026-10-17"));
        assert_eq!(parse_when("in 5 days", today()), Date::parse("2026-09-22"));
        assert_eq!(parse_when("2027-01-01", today()), Date::parse("2027-01-01"));
        assert_eq!(parse_when("someday", today()), None);
    }

    #[test]
    fn trial_takes_a_date_in_the_middle_of_the_phrase() {
        let spec = parse_trial("AMC may 15 2027 card Bank of America CREDIT", today()).unwrap();
        assert_eq!(spec.name, "AMC");
        assert_eq!(spec.trial_ends, Date::parse("2027-05-15"));
        assert_eq!(spec.card.as_deref(), Some("Bank of America CREDIT"));
        assert_eq!(spec.price, None);
    }

    #[test]
    fn trial_handles_multi_word_names_and_other_date_forms() {
        assert_eq!(parse_trial("Apple TV+ 2027-05-15", today()).unwrap().name, "Apple TV+");
        assert_eq!(parse_trial("New York Times +14d", today()).unwrap().name, "New York Times");
        assert_eq!(
            parse_trial("Disney Plus december 1", today()).unwrap().trial_ends,
            Date::parse("2026-12-01")
        );
    }

    #[test]
    fn trial_without_a_date_says_so() {
        let err = parse_trial("AMC", today()).unwrap_err();
        assert!(err.contains("cancel-by date"), "{err}");
        assert!(parse_trial("", today()).is_err());
    }

    #[test]
    fn edit_splits_target_field_and_value() {
        assert_eq!(
            parse_edit("netflix price 21.99").unwrap(),
            ("netflix".to_string(), "price".to_string(), "21.99".to_string())
        );
        assert_eq!(
            parse_edit("amazon prime cycle yearly").unwrap(),
            ("amazon prime".to_string(), "cycle".to_string(), "yearly".to_string())
        );
        assert_eq!(
            parse_edit("netflix note shared with family").unwrap().2,
            "shared with family"
        );
        assert!(parse_edit("netflix").is_err());
        assert!(parse_edit("price 21.99").is_err(), "no target");
        assert!(parse_edit("netflix price").is_err(), "no value");
    }

    #[test]
    fn verb_splitting() {
        assert_eq!(split_verb("renew netflix"), ("renew".to_string(), "netflix"));
        assert_eq!(split_verb("  LIST  "), ("list".to_string(), ""));
        assert_eq!(split_verb(""), ("".to_string(), ""));
        assert_eq!(split_verb("add Netflix 19.99 monthly").1, "Netflix 19.99 monthly");
    }

    #[test]
    fn ai_json_survives_fences_and_prose() {
        let reply = "Sure! Here's the data:\n```json\n{\"name\":\"Netflix Premium\",\
                     \"price\":22.99,\"cycle\":\"monthly\",\"next_due\":\"2026-10-03\",\
                     \"category\":\"streaming\"}\n```\nHope that helps.";
        let spec = AddSpec::from_json(reply, today()).unwrap();
        assert_eq!(spec.name, "Netflix Premium");
        assert_eq!(spec.price, Some(22.99));
        assert_eq!(spec.cycle, Cycle::MONTHLY);
        assert_eq!(spec.next_due, Date::parse("2026-10-03"));
        assert_eq!(spec.category.as_deref(), Some("streaming"));
    }

    #[test]
    fn ai_json_accepts_a_string_price_and_odd_cycle_words() {
        let spec = AddSpec::from_json(
            r#"{"name":"Figma","price":"$144.00","cycle":"annual"}"#,
            today(),
        )
        .unwrap();
        assert_eq!(spec.price, Some(144.0));
        assert_eq!(spec.cycle, Cycle::YEARLY);
        assert_eq!(spec.next_due, None);
    }

    #[test]
    fn ai_json_failures_are_reported_not_guessed() {
        assert!(AddSpec::from_json("", today()).is_err());
        assert!(AddSpec::from_json("no json here", today()).is_err());
        assert!(AddSpec::from_json("{\"price\":5}", today()).is_err(), "no name");
        let err = AddSpec::from_json("{oops}", today()).unwrap_err();
        assert!(err.contains("invalid JSON"), "{err}");
    }

    #[test]
    fn ai_json_defaults_the_cycle_when_the_model_omits_it() {
        let spec = AddSpec::from_json(r#"{"name":"Hulu","price":9.99}"#, today()).unwrap();
        assert_eq!(spec.cycle, Cycle::MONTHLY);
    }

    #[test]
    fn ai_json_keeps_an_unknown_price_unknown() {
        // Signing up for a trial rarely tells you what it converts to.
        let spec = AddSpec::from_json(
            r#"{"name":"AMC","price":null,"cycle":"monthly","trial_ends":"2027-05-15"}"#,
            today(),
        )
        .unwrap();
        assert_eq!(spec.price, None);
        assert_eq!(spec.trial_ends, Date::parse("2027-05-15"));
    }

    #[test]
    fn a_bare_date_is_the_end_date() {
        let spec = parse_add("Netflix 19.99 monthly may 15 2027", today()).unwrap();
        assert_eq!(spec.name, "Netflix", "the date is not part of the name");
        assert_eq!(spec.price, Some(19.99));
        assert_eq!(spec.next_due, Date::parse("2027-05-15"));
        assert_eq!(spec.trial_ends, None, "a priced entry is a paid renewal");

        let spec = parse_add("Hulu 9.99 monthly 2026-12-01", today()).unwrap();
        assert_eq!(spec.name, "Hulu");
        assert_eq!(spec.next_due, Date::parse("2026-12-01"));
    }

    #[test]
    fn a_date_with_no_price_is_carried_as_the_date() {
        // Routing only. `add_from_spec` decides this is a trial.
        let spec = parse_add("AMC may 15 2027", today()).unwrap();
        assert_eq!(spec.name, "AMC");
        assert_eq!(spec.price, None);
        assert_eq!(spec.next_due, Date::parse("2027-05-15"));
        assert!(!spec.is_trial, "nothing in the line said trial");
    }

    #[test]
    fn a_bare_trial_marker_claims_the_date() {
        let spec = parse_add("Netflix 19.99 monthly 2027-05-15 trial", today()).unwrap();
        assert!(spec.is_trial, "the marker wins over the price");
        assert_eq!(spec.trial_ends, Date::parse("2027-05-15"));
    }

    #[test]
    fn an_explicit_trial_flag_leaves_a_second_date_as_the_renewal() {
        let spec = parse_add("Hulu 9.99 monthly trial 2026-10-01 next 2026-11-01", today()).unwrap();
        assert_eq!(spec.trial_ends, Date::parse("2026-10-01"));
        assert_eq!(spec.next_due, Date::parse("2026-11-01"));
    }

    #[test]
    fn add_works_without_a_price() {
        let spec = parse_add("AMC trial may 15 2027 card Bank of America credit", today()).unwrap();
        assert_eq!(spec.name, "AMC");
        assert_eq!(spec.price, None);
        assert_eq!(spec.cycle, Cycle::MONTHLY, "defaults when nothing says otherwise");
        assert_eq!(spec.trial_ends, Date::parse("2027-05-15"));
        assert_eq!(spec.card.as_deref(), Some("Bank of America credit"));
    }

    #[test]
    fn add_without_a_price_still_finds_a_trailing_cycle() {
        let spec = parse_add("Adobe CC yearly", today()).unwrap();
        assert_eq!(spec.name, "Adobe CC", "the cycle word is not part of the name");
        assert_eq!(spec.cycle, Cycle::YEARLY);
        assert_eq!(spec.price, None);

        let spec = parse_add("New York Times", today()).unwrap();
        assert_eq!(spec.name, "New York Times", "a name with no cycle word survives whole");
    }

    #[test]
    fn a_run_of_words_is_only_a_date_if_every_word_fits() {
        // The strictness that keeps a price and cycle out of the date.
        assert_eq!(parse_when("19.99 monthly may 15 2027", today()), None);
        assert_eq!(parse_when("monthly may 15 2027", today()), None);
        assert_eq!(parse_when("Netflix may 15 2027", today()), None);
        assert_eq!(parse_when("may 15 2027 extra", today()), None);
        assert_eq!(parse_when("may 15 2027", today()), Date::parse("2027-05-15"));
    }

    #[test]
    fn dates_written_the_way_people_write_them() {
        assert_eq!(parse_when("may 15 2027", today()), Date::parse("2027-05-15"));
        assert_eq!(parse_when("May 15, 2027", today()), Date::parse("2027-05-15"));
        assert_eq!(parse_when("15 May 2027", today()), Date::parse("2027-05-15"));
        assert_eq!(parse_when("Jan 3 2027", today()), Date::parse("2027-01-03"));
        assert_eq!(parse_when("december 31 2026", today()), Date::parse("2026-12-31"));
        assert_eq!(parse_when("May 1st 2027", today()), Date::parse("2027-05-01"));
        // No year: the next time that date comes around.
        assert_eq!(parse_when("december 1", today()), Date::parse("2026-12-01"));
        assert_eq!(parse_when("january 5", today()), Date::parse("2027-01-05"), "already past");
        // A day past the end of the month clamps to the last day.
        assert_eq!(parse_when("feb 31 2027", today()), Date::parse("2027-02-28"));
        assert_eq!(parse_when("blursday 40 2027", today()), None);
    }
}
