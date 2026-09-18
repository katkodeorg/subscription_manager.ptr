//! Markdown output for the popup result panel.
//!
//! The popup renders results through `ReactMarkdown` with a component map for
//! headings, lists, bold, italics, code, and links. There is no table plugin
//! (`PopupView.tsx:77`). Everything here is headings and bullet lists, which
//! read as plain text when `looksLikeMarkdown` does not fire.

use crate::date::{phrase_due, phrase_span, Date};
use crate::model::{DeadlineKind, Store, Sub};

/// Renewals this far out or nearer are called "due soon".
pub const DUE_SOON_DAYS: i64 = 7;

/// One bullet line, phrased for whichever deadline applies.
fn line(store: &Store, sub: &Sub, deadline: Option<(i64, DeadlineKind)>) -> String {
    let mut parts = vec![format!("**{}**", sub.name), store.price_label(sub)];
    match deadline {
        // A trial is about cancelling in time, not about paying on time.
        Some((days, DeadlineKind::Trial)) => {
            parts.push(format!(
                "**trial** {}",
                match days {
                    d if d < -1 => format!("ended {} days ago", -d),
                    -1 => "ended yesterday".to_string(),
                    0 => "ends today".to_string(),
                    1 => "ends tomorrow".to_string(),
                    d => format!("ends in {d} days"),
                }
            ));
            parts.push(format!("then {}", sub.cycle));
        }
        Some((days, DeadlineKind::Renewal)) => {
            parts.push(phrase_due(days));
            parts.push(sub.cycle.to_string());
        }
        None => {
            parts.push(format!("unreadable date \"{}\"", sub.next_due));
            parts.push(sub.cycle.to_string());
        }
    }
    format!("- {}", parts.join(" · "))
}

/// The main digest: what is overdue, what is next, and what it all costs.
pub fn list(store: &Store, today: Date) -> String {
    if store.subs.is_empty() {
        return "No subscriptions tracked yet.\n\nAdd one with `sub add Netflix 19.99 monthly`, \
                or select a receipt and run `sub track`."
            .to_string();
    }

    let rows = store.by_due(today);
    let mut out = String::new();

    let mut section = |title: &str, items: Vec<String>| {
        if !items.is_empty() {
            out.push_str(&format!("## {title}\n{}\n\n", items.join("\n")));
        }
    };

    let bucket = |keep: fn(i64) -> bool| -> Vec<String> {
        rows.iter()
            .filter(|(_, deadline)| deadline.is_some_and(|(days, _)| keep(days)))
            .map(|(sub, deadline)| line(store, sub, *deadline))
            .collect()
    };

    section("Overdue", bucket(|days| days < 0));
    section("Due soon", bucket(|days| (0..=DUE_SOON_DAYS).contains(&days)));
    section("Upcoming", bucket(|days| days > DUE_SOON_DAYS));
    section(
        "Unreadable dates",
        rows.iter()
            .filter(|(_, deadline)| deadline.is_none())
            .map(|(sub, deadline)| line(store, sub, *deadline))
            .collect(),
    );

    let paused: Vec<String> = store
        .subs
        .iter()
        .filter(|s| s.paused)
        .map(|s| format!("- **{}** {} · {}", s.name, store.price_label(s), s.cycle))
        .collect();
    section("Paused", paused);

    out.push_str(&totals_line(store));
    out.push_str("\n\nMark one paid with `sub renew <name>`, or push it out with `sub snooze <name> 3`.");
    out
}

/// The running cost line shared by the digest and the spend report.
pub fn totals_line(store: &Store) -> String {
    let monthly = store.monthly_total();
    let active = store.active().count();
    let paused = store.subs.len() - active;
    let mut line = format!(
        "**{}/mo** · {}/yr · {active} active",
        store.money(monthly),
        store.money(monthly * 12.0)
    );
    if paused > 0 {
        line.push_str(&format!(" · {paused} paused"));
    }
    let unpriced = store.unpriced();
    if unpriced > 0 {
        line.push_str(&format!(" · {unpriced} without a price"));
    }
    line
}

/// `sub spend`, totals, category split, budget status, and the worst offender.
pub fn spend(store: &Store, today: Date) -> String {
    if store.active().next().is_none() {
        return "Nothing active to total up. Add a subscription with `sub add <name> <price> <cycle>`."
            .to_string();
    }

    let monthly = store.monthly_total();
    let mut out = format!("## Spend\n{}\n\n", totals_line(store));

    out.push_str("## By category\n");
    for (name, amount) in store.by_category() {
        let share = if monthly > 0.0 { amount / monthly * 100.0 } else { 0.0 };
        out.push_str(&format!(
            "- **{name}** {}/mo · {share:.0}%\n",
            store.money(amount)
        ));
    }

    if let Some(budget) = store.budget_monthly {
        let delta = monthly - budget;
        out.push_str(&format!(
            "\n## Budget\n{} of {}/mo, {}\n",
            store.money(monthly),
            store.money(budget),
            if delta > 0.0 {
                format!("**over by {}**", store.money(delta))
            } else {
                format!("{} to spare", store.money(-delta))
            }
        ));
    }

    // Biggest single line item, normalised per month so cycles compare fairly.
    if let Some(top) = store
        .active()
        .max_by(|a, b| a.monthly_cost().total_cmp(&b.monthly_cost()))
    {
        out.push_str(&format!(
            "\nLargest: **{}** at {}/mo ({} {}).\n",
            top.name,
            store.money(top.monthly_cost()),
            store.price_label(top),
            top.cycle
        ));
    }

    // Lifetime recorded spend, which only exists once renewals are logged.
    let paid: f64 = store.subs.iter().map(Sub::total_paid).sum();
    if paid > 0.0 {
        let count: usize = store.subs.iter().map(|s| s.history.len()).sum();
        out.push_str(&format!(
            "Recorded renewals: {} payment{} totalling {}.\n",
            count,
            if count == 1 { "" } else { "s" },
            store.money(paid)
        ));
    }

    let next = store
        .by_due(today)
        .into_iter()
        .find(|(_, deadline)| deadline.is_some_and(|(days, _)| days >= 0));
    if let Some((sub, Some((days, _)))) = next {
        out.push_str(&format!("Next charge: **{}** {}.\n", sub.name, phrase_due(days)));
    }
    out
}

/// `sub trials`, free trials by cancel-by date, soonest first.
pub fn trials(store: &Store, today: Date) -> String {
    let mut rows: Vec<(&Sub, Option<i64>)> = store
        .subs
        .iter()
        .filter(|s| s.is_trial())
        .map(|s| (s, s.trial_date().map(|d| today.days_until(d))))
        .collect();
    if rows.is_empty() {
        return "No free trials tracked. Mark one with `sub edit <name> trial 2026-10-01`, \
                or add it with `sub add <name> <price> <cycle> trial +14d`."
            .to_string();
    }
    rows.sort_by_key(|(_, d)| d.unwrap_or(i64::MAX));

    let mut out = String::from("## Free trials\n");
    for (sub, days) in rows {
        let when = match days {
            Some(d) if d < 0 => format!("**trial ended {} days ago**", -d),
            Some(0) => "**cancel today**".to_string(),
            Some(d) => format!("cancel within {d} days"),
            None => "no cancel-by date".to_string(),
        };
        out.push_str(&format!(
            "- **{}** {} · {} · then {} {}\n",
            sub.name,
            when,
            sub.trial_ends.as_deref().unwrap_or("?"),
            store.price_label(sub),
            sub.cycle
        ));
    }
    out.push_str("\nCancel a trial with `sub cancel <name>`, or keep it with `sub edit <name> trial none`.");
    out
}

/// `sub card <text>` finds everything billed to one card. Cards are free text,
/// such as "Bank of America credit" or "4242". The match is a case-insensitive
/// substring in both directions. "boa" finds "BoA credit". "4242" finds
/// "visa 4242".
pub fn by_card(store: &Store, query: &str) -> String {
    let needle = query.trim().trim_start_matches('*').to_lowercase();
    let hits: Vec<&Sub> = store
        .subs
        .iter()
        .filter(|s| {
            s.card
                .as_deref()
                .map(str::to_lowercase)
                .is_some_and(|c| c.contains(&needle) || needle.contains(&c))
        })
        .collect();
    if hits.is_empty() {
        let known: Vec<&str> = store.subs.iter().filter_map(|s| s.card.as_deref()).collect();
        return if known.is_empty() {
            "No cards recorded. Tag one with `sub edit netflix card BoA credit`.".to_string()
        } else {
            format!("Nothing is billed to \"{needle}\". Known cards: {}.", known.join(", "))
        };
    }
    let monthly: f64 = hits.iter().filter(|s| !s.paused).map(|s| s.monthly_cost()).sum();
    let mut out = format!("## Card: {needle}\n");
    for sub in &hits {
        out.push_str(&format!(
            "- **{}** {} {} · next {}{}\n",
            sub.name,
            store.price_label(sub),
            sub.cycle,
            sub.next_due,
            if sub.paused { " · paused" } else { "" }
        ));
    }
    out.push_str(&format!("\n**{}/mo** across {} subscriptions.", store.money(monthly), hits.len()));
    out
}

/// `sub show <name>`, everything known about one subscription.
pub fn detail(store: &Store, sub: &Sub, today: Date) -> String {
    let mut out = format!("## {}\n", sub.name);
    out.push_str(&match sub.price {
        Some(_) => format!(
            "- {} {} · {}/mo equivalent\n",
            store.price_label(sub),
            sub.cycle,
            store.money(sub.monthly_cost())
        ),
        None => format!(
            "- Price not recorded, {}\n", sub.cycle
        ),
    });
    // A trial's cancel-by date is its first charge date. Showing both prints
    // the same day twice.
    match (sub.trial_ends.as_deref(), sub.deadline(today)) {
        (Some(trial), Some((days, _))) => out.push_str(&format!(
            "- **Free trial, cancel by {trial}**, {}\n- It starts charging you that day\n",
            phrase_span(days)
        )),
        (Some(trial), None) => {
            out.push_str(&format!("- Free trial, cancel by {trial} (unreadable date)\n"))
        }
        (None, Some((days, _))) => {
            out.push_str(&format!("- Next renewal {}, {}\n", sub.next_due, phrase_due(days)))
        }
        (None, None) => {
            out.push_str(&format!("- Next renewal date unreadable: \"{}\"\n", sub.next_due))
        }
    }
    if sub.paused {
        out.push_str("- **Paused**, excluded from totals and due tracking\n");
    }
    if let Some(cat) = &sub.category {
        out.push_str(&format!("- Category: {cat}\n"));
    }
    if let Some(card) = &sub.card {
        out.push_str(&format!("- Card: {card}\n"));
    }
    if let Some(days) = sub.reminder_days {
        out.push_str(&format!("- Reminder {days} days before renewal\n"));
    }
    if sub.calendar_event_id.is_some() {
        out.push_str("- Calendar reminder created\n");
    }
    if let Some(note) = &sub.notes {
        out.push_str(&format!("- Note: {note}\n"));
    }
    if let Some(url) = &sub.cancel_url {
        out.push_str(&format!("- Cancel: {url}\n"));
    }
    // No command list. The popup matches extensions by keyword prefix, so
    // `sub cancel 1` typed into the bar matches nothing. A sentence goes
    // through the AI orchestrator and reaches the extension.
    out.push_str(&format!(
        "\nSay what you want: \"cancel {}\", \"{} is paid\", \"snooze {} a week\".\n",
        sub.name.to_lowercase(),
        sub.name.to_lowercase(),
        sub.name.to_lowercase()
    ));
    if !sub.history.is_empty() {
        out.push_str(&format!(
            "\n## Payments ({} payment{} totalling {})\n",
            sub.history.len(),
            if sub.history.len() == 1 { "" } else { "s" },
            store.money(sub.total_paid())
        ));
        // Newest first, capped at 12 lines.
        for payment in sub.history.iter().rev().take(12) {
            out.push_str(&format!("- {}, {}\n", payment.date, store.money(payment.amount)));
        }
        if sub.history.len() > 12 {
            out.push_str(&format!("- …and {} earlier\n", sub.history.len() - 12));
        }
    }
    out
}

pub fn help() -> String {
    "## Subscription Manager\n\
     - `subs`: what is overdue, due soon, and upcoming\n\
     - `sub add <name> [price] [cycle]`: e.g. `sub add Netflix 19.99 monthly`\n\
       options: `next <date>` `cat <x>` `card <x>` `url <link>` `trial <date>` `remind <days>` `note <x>`\n\
     - `sub trial <name> <date>`: a trial to cancel, e.g. `sub trial AMC may 15 2027`\n\
     - `sub track`: pull a subscription out of selected text with AI\n\
     - `sub renew <name>`: mark it paid and roll the date forward\n\
     - `sub snooze <name> [days]`: push the deadline without paying\n\
     - `sub pause` / `sub resume <name>`: stop or restart tracking\n\
     - `sub edit <name> <field> <value>`: price, cycle, next, cat, card, url, trial, note, name, remind, cal\n\
     - `sub remove <name>` / `sub undo`: delete, or take back the last change\n\
     - `sub show <name>`: full detail and payment history\n\
     - `sub spend`: monthly and yearly totals by category\n\
     - `sub trials`: free trials by cancel-by date\n\
     - `sub cancel <name>`: show the cancellation link (⌘C copies it)\n\
     - `sub card <text>`: everything on one card, e.g. `sub card BoA`\n\
     - `sub cal <name|all>`: create a Google Calendar reminder\n\
     - `sub export` / `sub import`: JSON backup and restore\n\
     - `sub budget <amount>` · `sub remind <days>` · `sub tz <hours>` · `sub currency <symbol>`\n\
     \n\
     Prices are optional. Dates accept `YYYY-MM-DD`, `may 15 2027`, `today`, `tomorrow`, `+10d`."
        .to_string()
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::date::Cycle;
    use crate::model::{new_sub, Payment};

    fn date(s: &str) -> Date {
        Date::parse(s).unwrap()
    }

    fn sample() -> Store {
        let mut store = Store::default();
        store.add(new_sub("Netflix".into(), Some(19.99), Cycle::MONTHLY, date("2026-09-14")));
        store.add(new_sub("Spotify".into(), Some(11.99), Cycle::MONTHLY, date("2026-09-19")));
        store.add(new_sub("Figma".into(), Some(144.0), Cycle::YEARLY, date("2027-01-10")));
        store
    }

    #[test]
    fn digest_groups_by_urgency_and_counts_days_since_the_deadline() {
        let out = list(&sample(), date("2026-09-17"));
        assert!(out.contains("## Overdue"), "{out}");
        assert!(out.contains("**Netflix**"));
        assert!(out.contains("due 3 days ago"), "{out}");
        assert!(out.contains("## Due soon"));
        assert!(out.contains("**Spotify**"));
        assert!(out.contains("due in 2 days"));
        assert!(out.contains("## Upcoming"));
        assert!(out.contains("**Figma**"));
        // Overdue must come before due-soon in the rendered order.
        assert!(out.find("## Overdue") < out.find("## Due soon"));
        assert!(out.find("## Due soon") < out.find("## Upcoming"));
    }

    #[test]
    fn digest_totals_normalise_cycles() {
        let out = list(&sample(), date("2026-09-17"));
        // 19.99 + 11.99 + 12.00
        assert!(out.contains("**$43.98/mo**"), "{out}");
        assert!(out.contains("$527.76/yr"), "{out}");
        assert!(out.contains("3 active"));
    }

    #[test]
    fn digest_is_markdown_the_popup_will_actually_render() {
        let out = list(&sample(), date("2026-09-17"));
        // Mirrors looksLikeMarkdown in PopupView.tsx: headings, bullets, bold.
        assert!(out.contains("\n- ") || out.starts_with("- "));
        assert!(out.contains("## "));
        assert!(out.contains("**"));
        assert!(!out.contains("|---"), "the popup has no table plugin");
    }

    #[test]
    fn empty_store_explains_how_to_start() {
        let out = list(&Store::default(), date("2026-09-17"));
        assert!(out.contains("sub add"), "{out}");
        assert!(out.contains("sub track"));
    }

    #[test]
    fn paused_subs_get_their_own_section_and_leave_the_total() {
        let mut store = sample();
        store.get_mut(3).unwrap().paused = true;
        let out = list(&store, date("2026-09-17"));
        assert!(out.contains("## Paused"), "{out}");
        assert!(out.contains("2 active · 1 paused"), "{out}");
        assert!(out.contains("**$31.98/mo**"), "Figma is excluded: {out}");
    }

    #[test]
    fn unreadable_dates_are_surfaced_not_hidden() {
        let mut store = sample();
        store.get_mut(2).unwrap().next_due = "soon".into();
        let out = list(&store, date("2026-09-17"));
        assert!(out.contains("## Unreadable dates"), "{out}");
        assert!(out.contains("unreadable date \"soon\""), "{out}");
    }

    #[test]
    fn an_imminent_trial_is_urgent_even_when_the_renewal_is_far_off() {
        let mut store = sample();
        // Renews in a month, but the trial ends in two days.
        store.add(new_sub("Audible".into(), Some(14.95), Cycle::MONTHLY, date("2026-10-17")));
        store.get_mut(4).unwrap().trial_ends = Some("2026-09-19".into());
        let out = list(&store, date("2026-09-17"));
        let due_soon = out.split("## Due soon").nth(1).unwrap();
        let upcoming_at = due_soon.find("## Upcoming").unwrap();
        assert!(
            due_soon[..upcoming_at].contains("**Audible**"),
            "an expiring trial belongs in Due soon, not Upcoming: {out}"
        );
        assert!(out.contains("**trial** ends in 2 days"), "{out}");
        assert!(out.contains("then monthly"), "{out}");
    }

    #[test]
    fn an_expired_trial_shows_up_as_overdue() {
        let mut store = Store::default();
        store.add(new_sub("Audible".into(), Some(14.95), Cycle::MONTHLY, date("2026-10-17")));
        store.get_mut(1).unwrap().trial_ends = Some("2026-09-10".into());
        let out = list(&store, date("2026-09-17"));
        assert!(out.contains("## Overdue"), "{out}");
        assert!(out.contains("**trial** ended 7 days ago"), "{out}");
    }

    #[test]
    fn spend_reports_categories_budget_and_the_biggest_line() {
        let mut store = sample();
        store.get_mut(1).unwrap().category = Some("streaming".into());
        store.get_mut(2).unwrap().category = Some("streaming".into());
        store.budget_monthly = Some(40.0);
        let out = spend(&store, date("2026-09-17"));
        assert!(out.contains("## By category"), "{out}");
        assert!(out.contains("**streaming** $31.98/mo"), "{out}");
        assert!(out.contains("**uncategorised**"));
        assert!(out.contains("## Budget"), "{out}");
        assert!(out.contains("over by $3.98"), "{out}");
        assert!(out.contains("Largest: **Netflix**"), "{out}");
        assert!(out.contains("Next charge: **Spotify** due in 2 days"), "{out}");
    }

    #[test]
    fn spend_reports_room_under_budget() {
        let mut store = sample();
        store.budget_monthly = Some(100.0);
        let out = spend(&store, date("2026-09-17"));
        assert!(out.contains("$56.02 to spare"), "{out}");
    }

    #[test]
    fn spend_includes_recorded_payment_history() {
        let mut store = sample();
        let sub = store.get_mut(1).unwrap();
        sub.history.push(Payment { date: "2026-08-14".into(), amount: 17.99 });
        sub.history.push(Payment { date: "2026-09-14".into(), amount: 19.99 });
        let out = spend(&store, date("2026-09-17"));
        assert!(out.contains("2 payments totalling $37.98"), "{out}");
    }

    #[test]
    fn trials_sort_by_cancel_by_date_and_flag_the_expired() {
        let mut store = sample();
        store.get_mut(1).unwrap().trial_ends = Some("2026-09-25".into());
        store.get_mut(2).unwrap().trial_ends = Some("2026-09-15".into());
        let out = trials(&store, date("2026-09-17"));
        assert!(out.find("Spotify") < out.find("Netflix"), "soonest first: {out}");
        assert!(out.contains("**trial ended 2 days ago**"), "{out}");
        assert!(out.contains("cancel within 8 days"), "{out}");
    }

    #[test]
    fn no_trials_explains_how_to_mark_one() {
        let out = trials(&sample(), date("2026-09-17"));
        assert!(out.contains("sub edit"), "{out}");
    }

    #[test]
    fn card_lookup_matches_on_last_four_and_totals_them() {
        let mut store = sample();
        store.get_mut(1).unwrap().card = Some("4242".into());
        store.get_mut(2).unwrap().card = Some("4242".into());
        store.get_mut(3).unwrap().card = Some("1881".into());
        let out = by_card(&store, "4242");
        assert!(out.contains("## Card: 4242"), "{out}");
        assert!(out.contains("**Netflix**") && out.contains("**Spotify**"));
        assert!(!out.contains("Figma"));
        assert!(out.contains("**$31.98/mo** across 2 subscriptions"), "{out}");

        let out = by_card(&store, "9999");
        assert!(out.contains("Known cards"), "{out}");

        // Free-text cards match either direction, case-insensitively.
        store.get_mut(3).unwrap().card = Some("Bank of America CREDIT".into());
        assert!(by_card(&store, "bank of america").contains("Figma"));
        assert!(by_card(&store, "Bank of America credit card").contains("Figma"));
        let out = by_card(&Store::default(), "4242");
        assert!(out.contains("No cards recorded"), "{out}");
    }

    #[test]
    fn detail_shows_everything_known_including_history() {
        let mut store = sample();
        let sub = store.get_mut(1).unwrap();
        sub.category = Some("streaming".into());
        sub.card = Some("4242".into());
        sub.cancel_url = Some("https://netflix.com/cancelplan".into());
        sub.notes = Some("joint account".into());
        sub.history.push(Payment { date: "2026-08-14".into(), amount: 17.99 });
        let sub = store.subs[0].clone();
        let out = detail(&store, &sub, date("2026-09-17"));
        assert!(out.contains("## Netflix"), "{out}");
        assert!(out.contains("due 3 days ago"), "{out}");
        assert!(out.contains("streaming") && out.contains("Card: 4242"));
        assert!(out.contains("joint account"));
        assert!(out.contains("https://netflix.com/cancelplan"));
        assert!(out.contains("## Payments (1 payment totalling $17.99)"), "{out}");
        // Inspect opens this view. It suggests plain English, since a command
        // with arguments typed into the bar matches no keyword.
        assert!(out.contains("Say what you want"), "{out}");
        assert!(out.contains("\"cancel netflix\""), "{out}");
        assert!(!out.contains("`sub renew 1`"), "un-typeable commands stay out: {out}");
    }

    #[test]
    fn detail_of_a_trial_offers_cancelling_and_the_price() {
        let mut store = Store::default();
        store.add(new_sub("AMC".into(), None, Cycle::MONTHLY, date("2027-05-15")));
        store.get_mut(1).unwrap().trial_ends = Some("2027-05-15".into());
        store.get_mut(1).unwrap().card = Some("Bank of America CREDIT".into());
        let sub = store.subs[0].clone();
        let out = detail(&store, &sub, date("2026-09-17"));
        assert!(out.contains("Price not recorded"), "{out}");
        assert!(out.contains("Bank of America CREDIT"), "{out}");
        assert!(out.contains("Free trial, cancel by 2027-05-15"), "{out}");
        assert_eq!(out.matches("2027-05-15").count(), 1, "the date is shown once: {out}");
        assert!(!out.contains("Next renewal"), "{out}");
        assert!(out.contains("Say what you want"), "{out}");
    }

    #[test]
    fn detail_caps_a_long_payment_history() {
        let mut store = sample();
        for i in 1..=20 {
            store.get_mut(1).unwrap().history.push(Payment {
                date: format!("2025-{:02}-01", (i % 12) + 1),
                amount: 10.0,
            });
        }
        let sub = store.subs[0].clone();
        let out = detail(&store, &sub, date("2026-09-17"));
        assert!(out.contains("and 8 earlier"), "{out}");
    }

    #[test]
    fn help_lists_the_commands_it_implements() {
        let out = help();
        for cmd in ["subs", "sub add", "sub renew", "sub snooze", "sub spend", "sub trials",
                    "sub cancel", "sub cal", "sub export", "sub undo", "sub tz"] {
            assert!(out.contains(cmd), "help is missing {cmd}");
        }
    }
}
