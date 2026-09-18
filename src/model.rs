//! The stored subscription list and the operations over it.
//!
//! One storage key, `data`, holds a single JSON document. The previous document
//! sits under `undo`. Storage is a flat file per key
//! (`community_ext/storage.rs`), so one document is one write.

use crate::date::{Cycle, Date};
use pointiv_extension_sdk::storage;
use serde::{Deserialize, Serialize};

pub const DATA_KEY: &str = "data";
pub const UNDO_KEY: &str = "undo";
pub const SCHEMA_VERSION: u32 = 1;

/// Which date a subscription is being judged against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeadlineKind {
    /// A free trial's cancel-by date: no money has been spent yet.
    Trial,
    Renewal,
}

/// One recorded renewal payment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Payment {
    pub date: String,
    pub amount: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sub {
    pub id: u32,
    pub name: String,
    /// `None` when the price is unknown. Common for a free trial.
    #[serde(default)]
    pub price: Option<f64>,
    pub cycle: Cycle,
    /// `YYYY-MM-DD`. The single source of truth for "when does this renew".
    pub next_due: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    /// Last four digits of the card, so an expiring card can be traced.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub card: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cancel_url: Option<String>,
    /// Set means this is a free trial and this is the cancel-by date.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trial_ends: Option<String>,
    #[serde(default)]
    pub paused: bool,
    /// Days before renewal for the calendar reminder; falls back to the store default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reminder_days: Option<u32>,
    /// Google Calendar event id, so a second `sub cal` does not duplicate it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calendar_event_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub history: Vec<Payment>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

impl Sub {
    pub fn due_date(&self) -> Option<Date> {
        Date::parse(&self.next_due)
    }

    pub fn trial_date(&self) -> Option<Date> {
        self.trial_ends.as_deref().and_then(Date::parse)
    }

    pub fn is_trial(&self) -> bool {
        self.trial_ends.is_some()
    }

    /// Cost per month, for totals that mix cycles. An unknown price counts as
    /// zero. The totals line reports how many of those there are.
    pub fn monthly_cost(&self) -> f64 {
        self.price.unwrap_or(0.0) * self.cycle.monthly_factor()
    }

    pub fn has_price(&self) -> bool {
        self.price.is_some()
    }

    /// Signed days from `today` to the renewal date. `None` when the stored
    /// date will not parse. The digest shows those in their own section.
    pub fn days_until(&self, today: Date) -> Option<i64> {
        self.due_date().map(|due| today.days_until(due))
    }

    /// The deadline that matters, and how far off it is.
    ///
    /// A trial's cancel-by date wins over the renewal date. Missing it turns
    /// the trial into a charge. The tile and the digest both order by this.
    pub fn deadline(&self, today: Date) -> Option<(i64, DeadlineKind)> {
        if let Some(trial) = self.trial_date() {
            return Some((today.days_until(trial), DeadlineKind::Trial));
        }
        self.days_until(today).map(|days| (days, DeadlineKind::Renewal))
    }

    /// What the last renewal actually cost, for price-change detection.
    pub fn last_paid(&self) -> Option<&Payment> {
        self.history.last()
    }

    pub fn total_paid(&self) -> f64 {
        self.history.iter().map(|p| p.amount).sum()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Store {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default = "default_next_id")]
    pub next_id: u32,
    /// Most recent date the extension has observed, and the fallback if the
    /// sandbox clock ever stops answering. Written only when it changes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seen: Option<String>,
    #[serde(default = "default_currency")]
    pub currency: String,
    /// Minutes to add to UTC before taking the date. The sandbox exposes no
    /// timezone, so `sub tz` lets the user align "today" with their own day.
    #[serde(default)]
    pub tz_offset_minutes: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_monthly: Option<f64>,
    #[serde(default = "default_reminder_days")]
    pub default_reminder_days: u32,
    #[serde(default)]
    pub subs: Vec<Sub>,
}

fn default_version() -> u32 {
    SCHEMA_VERSION
}
fn default_next_id() -> u32 {
    1
}
fn default_currency() -> String {
    "$".to_string()
}
fn default_reminder_days() -> u32 {
    3
}

impl Default for Store {
    fn default() -> Self {
        Store {
            version: SCHEMA_VERSION,
            next_id: 1,
            last_seen: None,
            currency: default_currency(),
            tz_offset_minutes: 0,
            budget_monthly: None,
            default_reminder_days: default_reminder_days(),
            subs: Vec::new(),
        }
    }
}

/// Why a name or id did not resolve to exactly one subscription.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveError {
    Empty,
    NotFound(String),
    /// More than one candidate: the display names, for an actionable message.
    Ambiguous(String, Vec<String>),
}

impl std::fmt::Display for ResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResolveError::Empty => write!(f, "Name a subscription, e.g. `sub renew netflix`."),
            ResolveError::NotFound(q) => {
                write!(f, "No subscription matches \"{q}\". Run `subs` to see the list.")
            }
            ResolveError::Ambiguous(q, names) => write!(
                f,
                "\"{q}\" matches {}. Use a longer name or the id.",
                names.join(", ")
            ),
        }
    }
}

impl Store {
    pub fn load() -> Store {
        storage::read_json::<Store>(DATA_KEY).unwrap_or_default()
    }

    pub fn save(&self) {
        storage::write_json(DATA_KEY, self);
    }

    /// Snapshot the current document before a mutation, for `sub undo`. Reads
    /// the on-disk state, not `self`. An in-memory mutation already applied
    /// stays out of the snapshot.
    pub fn snapshot_for_undo() {
        match storage::read(DATA_KEY) {
            Some(raw) => storage::write(UNDO_KEY, &raw),
            None => storage::write(UNDO_KEY, "{}"),
        }
    }

    /// Restore the undo snapshot. Returns false when there is nothing to undo.
    pub fn undo() -> bool {
        match storage::read(UNDO_KEY) {
            Some(raw) if raw.trim() != "{}" && !raw.trim().is_empty() => {
                // Swap, so undo is itself undoable.
                let current = storage::read(DATA_KEY).unwrap_or_else(|| "{}".to_string());
                storage::write(DATA_KEY, &raw);
                storage::write(UNDO_KEY, &current);
                true
            }
            _ => false,
        }
    }

    pub fn add(&mut self, mut sub: Sub) -> u32 {
        let id = self.next_id;
        sub.id = id;
        self.next_id += 1;
        self.subs.push(sub);
        id
    }

    pub fn remove(&mut self, id: u32) -> Option<Sub> {
        let idx = self.subs.iter().position(|s| s.id == id)?;
        Some(self.subs.remove(idx))
    }

    pub fn get_mut(&mut self, id: u32) -> Option<&mut Sub> {
        self.subs.iter_mut().find(|s| s.id == id)
    }

    /// Resolve a user-typed token to one subscription id.
    ///
    /// Tries a numeric id, then an exact name, then a unique prefix, then a
    /// unique substring. Names are case-insensitive. Ids never shift, so a tile
    /// button from an earlier render still targets the right row.
    pub fn resolve(&self, query: &str) -> Result<u32, ResolveError> {
        let q = query.trim();
        if q.is_empty() {
            return Err(ResolveError::Empty);
        }
        if let Ok(id) = q.parse::<u32>()
            && self.subs.iter().any(|s| s.id == id) {
                return Ok(id);
            }
        let ql = q.to_lowercase();

        if let Some(s) = self.subs.iter().find(|s| s.name.to_lowercase() == ql) {
            return Ok(s.id);
        }
        for matcher in [
            |name: &str, q: &str| name.starts_with(q),
            |name: &str, q: &str| name.contains(q),
        ] {
            let hits: Vec<&Sub> = self
                .subs
                .iter()
                .filter(|s| matcher(&s.name.to_lowercase(), &ql))
                .collect();
            match hits.len() {
                1 => return Ok(hits[0].id),
                0 => continue,
                _ => {
                    return Err(ResolveError::Ambiguous(
                        q.to_string(),
                        hits.iter().map(|s| s.name.clone()).collect(),
                    ));
                }
            }
        }
        Err(ResolveError::NotFound(q.to_string()))
    }

    /// Subscriptions that count toward spend and due tracking.
    pub fn active(&self) -> impl Iterator<Item = &Sub> {
        self.subs.iter().filter(|s| !s.paused)
    }

    pub fn monthly_total(&self) -> f64 {
        self.active().map(|s| s.monthly_cost()).sum()
    }

    /// Active subscriptions ordered by the deadline that matters, most urgent
    /// first. An entry whose date will not parse sorts last.
    pub fn by_due(&self, today: Date) -> Vec<(&Sub, Option<(i64, DeadlineKind)>)> {
        let mut rows: Vec<(&Sub, Option<(i64, DeadlineKind)>)> =
            self.active().map(|s| (s, s.deadline(today))).collect();
        rows.sort_by(|a, b| match (a.1, b.1) {
            (Some((x, _)), Some((y, _))) => x.cmp(&y).then_with(|| a.0.name.cmp(&b.0.name)),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => a.0.name.cmp(&b.0.name),
        });
        rows
    }

    /// Monthly spend per category, largest first. Untagged entries roll up
    /// under "uncategorised".
    pub fn by_category(&self) -> Vec<(String, f64)> {
        let mut acc: Vec<(String, f64)> = Vec::new();
        for sub in self.active() {
            let key = sub
                .category
                .as_deref()
                .map(str::to_lowercase)
                .unwrap_or_else(|| "uncategorised".to_string());
            match acc.iter_mut().find(|(k, _)| *k == key) {
                Some(entry) => entry.1 += sub.monthly_cost(),
                None => acc.push((key, sub.monthly_cost())),
            }
        }
        acc.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        acc
    }

    pub fn reminder_days_for(&self, sub: &Sub) -> u32 {
        sub.reminder_days.unwrap_or(self.default_reminder_days)
    }

    /// Format an amount in the store's currency.
    pub fn money(&self, amount: f64) -> String {
        format!("{}{:.2}", self.currency, amount)
    }

    /// A subscription's price for display, or a marker when it is unknown.
    pub fn price_label(&self, sub: &Sub) -> String {
        match sub.price {
            Some(price) => self.money(price),
            None => "price ?".to_string(),
        }
    }

    /// How many active subscriptions have no price recorded.
    pub fn unpriced(&self) -> usize {
        self.active().filter(|s| !s.has_price()).count()
    }
}

/// A subscription built from parsed input, before it gets an id.
pub fn new_sub(name: String, price: Option<f64>, cycle: Cycle, next_due: Date) -> Sub {
    Sub {
        id: 0,
        name,
        price,
        cycle,
        next_due: next_due.to_string(),
        category: None,
        card: None,
        cancel_url: None,
        trial_ends: None,
        paused: false,
        reminder_days: None,
        calendar_event_id: None,
        history: Vec::new(),
        notes: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(s: &str) -> Date {
        Date::parse(s).unwrap()
    }

    fn store_with(names: &[(&str, f64, Cycle, &str)]) -> Store {
        let mut store = Store::default();
        for (name, price, cycle, due) in names {
            store.add(new_sub(name.to_string(), Some(*price), *cycle, date(due)));
        }
        store
    }

    #[test]
    fn ids_are_stable_across_removal() {
        let mut store = store_with(&[
            ("Netflix", 19.99, Cycle::MONTHLY, "2026-10-03"),
            ("Spotify", 11.99, Cycle::MONTHLY, "2026-09-28"),
            ("Figma", 144.0, Cycle::YEARLY, "2027-01-10"),
        ]);
        assert_eq!(store.resolve("Spotify"), Ok(2));
        store.remove(1);
        // Spotify keeps id 2: a tile button minted before the removal still
        // still targets Spotify. It does not slide onto another row.
        assert_eq!(store.resolve("Spotify"), Ok(2));
        assert_eq!(store.resolve("Figma"), Ok(3));
        // And a new subscription never reuses a freed id.
        let id = store.add(new_sub("Hulu".into(), Some(9.99), Cycle::MONTHLY, date("2026-10-01")));
        assert_eq!(id, 4);
    }

    #[test]
    fn resolve_prefers_id_then_exact_then_prefix_then_substring() {
        let store = store_with(&[
            ("Netflix", 19.99, Cycle::MONTHLY, "2026-10-03"),
            ("Net Solutions", 5.0, Cycle::MONTHLY, "2026-10-05"),
            ("Amazon Prime", 14.99, Cycle::MONTHLY, "2026-10-09"),
        ]);
        assert_eq!(store.resolve("1"), Ok(1));
        assert_eq!(store.resolve("netflix"), Ok(1), "exact beats the ambiguous prefix");
        assert_eq!(store.resolve("NETFLIX"), Ok(1), "case-insensitive");
        assert_eq!(store.resolve("netf"), Ok(1), "unique prefix");
        assert_eq!(store.resolve("prime"), Ok(3), "unique substring");
        assert_eq!(
            store.resolve("net"),
            Err(ResolveError::Ambiguous(
                "net".into(),
                vec!["Netflix".into(), "Net Solutions".into()]
            )),
            "an ambiguous prefix asks"
        );
        assert_eq!(store.resolve("hulu"), Err(ResolveError::NotFound("hulu".into())));
        assert_eq!(store.resolve("  "), Err(ResolveError::Empty));
        assert_eq!(store.resolve("99"), Err(ResolveError::NotFound("99".into())));
    }

    #[test]
    fn monthly_total_normalises_mixed_cycles() {
        let store = store_with(&[
            ("Netflix", 20.0, Cycle::MONTHLY, "2026-10-03"),
            ("Figma", 120.0, Cycle::YEARLY, "2027-01-10"),
            ("Laundry", 30.0, Cycle::QUARTERLY, "2026-11-01"),
        ]);
        // 20 + 10 + 10
        assert!((store.monthly_total() - 40.0).abs() < 0.01);
    }

    #[test]
    fn paused_subs_leave_totals_and_due_list() {
        let mut store = store_with(&[
            ("Netflix", 20.0, Cycle::MONTHLY, "2026-10-03"),
            ("Hulu", 10.0, Cycle::MONTHLY, "2026-09-20"),
        ]);
        store.get_mut(2).unwrap().paused = true;
        assert!((store.monthly_total() - 20.0).abs() < 0.01);
        let rows = store.by_due(date("2026-09-17"));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0.name, "Netflix");
    }

    #[test]
    fn by_due_puts_the_most_overdue_first() {
        let store = store_with(&[
            ("Upcoming", 1.0, Cycle::MONTHLY, "2026-10-01"),
            ("VeryLate", 1.0, Cycle::MONTHLY, "2026-09-01"),
            ("JustLate", 1.0, Cycle::MONTHLY, "2026-09-14"),
            ("Today", 1.0, Cycle::MONTHLY, "2026-09-17"),
        ]);
        let rows = store.by_due(date("2026-09-17"));
        let order: Vec<&str> = rows.iter().map(|(s, _)| s.name.as_str()).collect();
        assert_eq!(order, vec!["VeryLate", "JustLate", "Today", "Upcoming"]);
        assert_eq!(rows[0].1, Some((-16, DeadlineKind::Renewal)));
        assert_eq!(rows[1].1, Some((-3, DeadlineKind::Renewal)), "the \"due 3 days ago\" line");
        assert_eq!(rows[2].1, Some((0, DeadlineKind::Renewal)));
        assert_eq!(rows[3].1, Some((14, DeadlineKind::Renewal)));
    }

    #[test]
    fn unparseable_due_date_sorts_last_instead_of_panicking() {
        let mut store = store_with(&[("Good", 1.0, Cycle::MONTHLY, "2026-10-01")]);
        let mut broken = new_sub("Broken".into(), Some(1.0), Cycle::MONTHLY, date("2026-10-01"));
        broken.next_due = "whenever".into();
        store.add(broken);
        let rows = store.by_due(date("2026-09-17"));
        assert_eq!(rows[0].0.name, "Good");
        assert_eq!(rows[1].0.name, "Broken");
        assert_eq!(rows[1].1, None);
    }

    #[test]
    fn categories_roll_up_and_sort_by_spend() {
        let mut store = store_with(&[
            ("Netflix", 20.0, Cycle::MONTHLY, "2026-10-03"),
            ("Hulu", 10.0, Cycle::MONTHLY, "2026-10-04"),
            ("Figma", 120.0, Cycle::YEARLY, "2027-01-10"),
            ("Mystery", 5.0, Cycle::MONTHLY, "2026-10-08"),
        ]);
        store.get_mut(1).unwrap().category = Some("Streaming".into());
        store.get_mut(2).unwrap().category = Some("streaming".into());
        store.get_mut(3).unwrap().category = Some("design".into());
        let cats = store.by_category();
        assert_eq!(cats[0].0, "streaming", "case-folded into one bucket");
        assert!((cats[0].1 - 30.0).abs() < 0.01);
        assert_eq!(cats[1].0, "design");
        assert_eq!(cats[2].0, "uncategorised");
    }

    #[test]
    fn an_unknown_price_counts_as_zero_and_is_reported_separately() {
        let mut store = store_with(&[("Netflix", 20.0, Cycle::MONTHLY, "2026-10-03")]);
        store.add(new_sub("AMC".into(), None, Cycle::MONTHLY, date("2027-05-15")));
        assert!((store.monthly_total() - 20.0).abs() < 0.01, "an unknown price counts as zero");
        assert_eq!(store.unpriced(), 1);
        assert_eq!(store.price_label(&store.subs[1]), "price ?");
        assert_eq!(store.price_label(&store.subs[0]), "$20.00");
    }

    #[test]
    fn a_priceless_subscription_round_trips() {
        let mut store = Store::default();
        store.add(new_sub("AMC".into(), None, Cycle::MONTHLY, date("2027-05-15")));
        let json = serde_json::to_string(&store).unwrap();
        let back: Store = serde_json::from_str(&json).unwrap();
        assert_eq!(back.subs[0].price, None);
        assert!(!back.subs[0].has_price());
    }

    #[test]
    fn money_uses_the_store_currency() {
        let mut store = Store::default();
        assert_eq!(store.money(19.9), "$19.90");
        store.currency = "£".into();
        assert_eq!(store.money(5.0), "£5.00");
    }

    #[test]
    fn a_trial_cancel_by_date_outranks_the_renewal_date() {
        let mut store = store_with(&[
            ("Audible", 14.95, Cycle::MONTHLY, "2026-10-17"),
            ("Netflix", 19.99, Cycle::MONTHLY, "2026-09-20"),
        ]);
        store.get_mut(1).unwrap().trial_ends = Some("2026-09-19".into());
        let today = date("2026-09-17");
        assert_eq!(store.subs[0].deadline(today), Some((2, DeadlineKind::Trial)));
        assert_eq!(store.subs[1].deadline(today), Some((3, DeadlineKind::Renewal)));
        // Audible renews a month out, but its trial ends first, so it leads.
        let rows = store.by_due(today);
        assert_eq!(rows[0].0.name, "Audible");
    }

    #[test]
    fn history_tracks_spend_and_the_last_price() {
        let mut store = store_with(&[("Netflix", 19.99, Cycle::MONTHLY, "2026-10-03")]);
        let sub = store.get_mut(1).unwrap();
        sub.history.push(Payment { date: "2026-08-03".into(), amount: 17.99 });
        sub.history.push(Payment { date: "2026-09-03".into(), amount: 19.99 });
        assert!((sub.total_paid() - 37.98).abs() < 0.01);
        assert_eq!(sub.last_paid().unwrap().amount, 19.99);
    }

    #[test]
    fn store_json_round_trips_and_tolerates_missing_fields() {
        let store = store_with(&[("Netflix", 19.99, Cycle::MONTHLY, "2026-10-03")]);
        let json = serde_json::to_string(&store).unwrap();
        let back: Store = serde_json::from_str(&json).unwrap();
        assert_eq!(back.subs.len(), 1);
        assert_eq!(back.subs[0].name, "Netflix");
        assert_eq!(back.next_id, 2);

        // A minimal document, the shape `sub import` may receive, must fill in.
        let sparse: Store = serde_json::from_str(
            r#"{"subs":[{"id":1,"name":"X","price":1.0,"cycle":{"months":1},"next_due":"2026-10-01"}]}"#,
        )
        .unwrap();
        assert_eq!(sparse.version, SCHEMA_VERSION);
        assert_eq!(sparse.currency, "$");
        assert_eq!(sparse.default_reminder_days, 3);
        assert!(!sparse.subs[0].paused);
        assert!(sparse.subs[0].history.is_empty());
    }

    #[test]
    fn empty_optionals_stay_out_of_the_json() {
        let store = store_with(&[("Netflix", 19.99, Cycle::MONTHLY, "2026-10-03")]);
        let json = serde_json::to_string(&store).unwrap();
        assert!(!json.contains("category"), "unset options are omitted: {json}");
        assert!(!json.contains("history"));
        assert!(!json.contains("last_seen"));
    }

    #[test]
    fn reminder_days_fall_back_to_the_store_default() {
        let mut store = store_with(&[("Netflix", 19.99, Cycle::MONTHLY, "2026-10-03")]);
        let sub = store.subs[0].clone();
        assert_eq!(store.reminder_days_for(&sub), 3);
        store.default_reminder_days = 7;
        assert_eq!(store.reminder_days_for(&sub), 7);
        store.get_mut(1).unwrap().reminder_days = Some(1);
        let sub = store.subs[0].clone();
        assert_eq!(store.reminder_days_for(&sub), 1);
    }
}
