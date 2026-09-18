//! Subscription Manager, a Pointiv community extension.
//!
//! The platform calls two exports:
//!
//! * `execute` handles typed commands and tile button presses. It has storage,
//!   AI, and Google Calendar access.
//! * `render_tile` draws the tile beside the popup command bar. It has storage
//!   access and a 3 second budget.
//!
//! A typed command is parsed here. A sentence arrives from the AI orchestrator,
//! and the model maps it to one of the same commands. Both run the same code,
//! so every mutation has one implementation.

pub mod date;
pub mod digest;
pub mod model;
pub mod parse;
pub mod render;

use date::{Cycle, Date};
use model::{new_sub, Payment, Store, Sub};
use parse::AddSpec;
use pointiv_extension_sdk::prelude::*;

/// "1 payment", "2 payments".
fn plural(count: usize, noun: &str) -> String {
    if count == 1 {
        format!("{count} {noun}")
    } else {
        format!("{count} {noun}s")
    }
}

/// Google rejects reminder offsets beyond 4 weeks.
const MAX_REMINDER_DAYS: u32 = 28;
/// Default push for `sub snooze` with no day count.
const DEFAULT_SNOOZE_DAYS: i64 = 3;

// ── Clock ────────────────────────────────────────────────────────────────────

/// Convert an epoch timestamp to a local calendar date.
///
/// Split from the clock call so tests can pass a fixed timestamp. The sandbox
/// exposes no timezone. `tz_offset_minutes`, set by `sub tz`, shifts the date
/// from UTC to the user's day.
fn date_from_epoch_secs(secs: i64, tz_offset_minutes: i32) -> Option<Date> {
    let shifted = secs.checked_add(tz_offset_minutes as i64 * 60)?;
    let date = Date::from_epoch_day(shifted.div_euclid(86_400));
    // A plausible year means the clock answered; anything else is a broken
    // host and we fall back to the last date we saw.
    (2024..=2100).contains(&date.y).then_some(date)
}

/// Today's date from the sandbox clock. Verified to work under Extism's WASI.
fn clock_date(tz_offset_minutes: i32) -> Option<Date> {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs() as i64;
    date_from_epoch_secs(secs, tz_offset_minutes)
}

/// Today, preferring the clock and falling back to the last date observed.
fn resolve_today(store: &Store) -> Option<Date> {
    today_from(clock_date(store.tz_offset_minutes), store)
}

/// The fallback chain, split out from the clock call so it can be tested with
/// the clock forced to fail.
fn today_from(clock: Option<Date>, store: &Store) -> Option<Date> {
    clock.or_else(|| store.last_seen.as_deref().and_then(Date::parse))
}

/// Record the date if it moved. Returns true when the store needs saving.
fn touch_last_seen(store: &mut Store, today: Date) -> bool {
    let stamp = today.to_string();
    if store.last_seen.as_deref() == Some(stamp.as_str()) {
        return false;
    }
    store.last_seen = Some(stamp);
    true
}

const NO_DATE: &str = "I can't determine today's date, so renewal timing is unavailable. \
                       Open the Pointiv popup once and try again.";

// ── Exports ──────────────────────────────────────────────────────────────────

#[plugin_fn]
pub fn execute(Json(input): Json<Input>) -> FnResult<Json<Output>> {
    let mut store = Store::load();
    let today = resolve_today(&store);
    if let Some(today) = today
        && touch_last_seen(&mut store, today) {
            store.save();
        }

    let command = strip_prefix(input.command.trim());
    log::info(&format!(
        "execute: command={:?} selection_chars={} subs={}",
        command,
        input.text.len(),
        store.subs.len()
    ));

    let (verb, args) = parse::split_verb(command);
    Ok(Json(show(dispatch(&mut store, &verb, args, &input, today, true))))
}

#[plugin_fn]
pub fn render_tile(Json(input): Json<TileRenderInput>) -> FnResult<Json<TileUi>> {
    let mut store = Store::load();
    // The host hands us an authoritative timestamp here. Persist the date when
    // it changes so `execute` keeps a fallback if the clock ever stops working.
    let today = Date::parse_rfc3339(&input.now)
        .map(|utc| utc.add_days(tz_day_shift(&store, &input.now)))
        .or_else(|| resolve_today(&store));
    if let Some(today) = today
        && touch_last_seen(&mut store, today) {
            store.save();
        }
    Ok(Json(render::tile(&store, today)))
}

/// Force a successful result to be displayed.
///
/// The popup drops a tile button's output unless it fails or carries a
/// sentinel (`PopupView.tsx`: "plain success stays quiet"). Without this,
/// Inspect does nothing. The popup strips `__OVERLAY__` before showing the
/// text, so ⌘C copies a clean payload.
fn show(output: Output) -> Output {
    if output.kind == "text" && !output.value.starts_with("__") {
        return Output { kind: output.kind, value: format!("__OVERLAY__\n{}", output.value) };
    }
    output
}

/// How many days the timezone offset moves the host's UTC date.
///
/// The tile's `now` carries a wall-clock time, so applying the offset needs the
/// hour, not just the date: 2026-09-17T23:30Z at UTC-7 is still the 17th.
fn tz_day_shift(store: &Store, now: &str) -> i64 {
    if store.tz_offset_minutes == 0 {
        return 0;
    }
    let minutes_into_day = now
        .split('T')
        .nth(1)
        .map(|time| {
            let mut parts = time.split(':');
            let h: i64 = parts.next().and_then(|h| h.parse().ok()).unwrap_or(0);
            let m: i64 = parts.next().and_then(|m| m.parse().ok()).unwrap_or(0);
            h * 60 + m
        })
        .unwrap_or(0);
    (minutes_into_day + store.tz_offset_minutes as i64).div_euclid(1440)
}

/// Drop a leading `sub`/`subs`/`subscription` token so both `subs` (typed in
/// the bar, where matching is prefix-based) and `sub renew 2` (minted by a tile
/// button) reach the same dispatcher.
fn strip_prefix(command: &str) -> &str {
    let lower = command.to_lowercase();
    for prefix in ["subscriptions", "subscription", "subs", "sub"] {
        if lower == prefix {
            return "";
        }
        if let Some(rest) = lower.strip_prefix(prefix)
            && rest.starts_with(char::is_whitespace) {
                return command[prefix.len()..].trim();
            }
    }
    command
}

// ── Dispatch ─────────────────────────────────────────────────────────────────

fn dispatch(
    store: &mut Store,
    verb: &str,
    args: &str,
    input: &Input,
    today: Option<Date>,
    ai_allowed: bool,
) -> Output {
    // Commands that need to know what day it is.
    macro_rules! today_or_bail {
        () => {
            match today {
                Some(t) => t,
                None => return Output::error(NO_DATE),
            }
        };
    }

    match verb {
        // A selection plus a bare verb tracks the selection. `list` and the
        // tile's All button send no selection and show the list.
        "" if !input.text.trim().is_empty() => {
            cmd_track(store, args, input, today_or_bail!(), ai_allowed)
        }
        "" | "list" | "ls" | "all" | "renewals" | "upcoming" => {
            Output::text(digest::list(store, today_or_bail!()))
        }
        "add" | "new" | "track" if verb == "track" || args.is_empty() => {
            cmd_track(store, args, input, today_or_bail!(), ai_allowed)
        }
        "add" | "new" => cmd_add(store, args, today_or_bail!(), ai_allowed),
        "renew" | "renewed" | "paid" | "done" => cmd_renew(store, args, today_or_bail!()),
        "snooze" | "defer" | "postpone" | "later" => cmd_snooze(store, args, today_or_bail!()),
        "pause" | "hold" => cmd_pause(store, args, true),
        "resume" | "unpause" | "restart" => cmd_pause(store, args, false),
        "edit" | "set" | "change" => cmd_edit(store, args, today_or_bail!()),
        "remove" | "rm" | "delete" | "forget" | "drop" => cmd_remove(store, args),
        "undo" => cmd_undo(),
        "show" | "info" | "detail" | "details" => cmd_show(store, args, today_or_bail!()),
        "spend" | "cost" | "costs" | "total" | "totals" | "summary" => {
            Output::text(digest::spend(store, today_or_bail!()))
        }
        // `sub trial AMC may 15 2027` adds one; bare `sub trial(s)` lists them.
        "trial" | "trials" if !args.is_empty() => cmd_trial(store, args, today_or_bail!()),
        "trial" | "trials" => Output::text(digest::trials(store, today_or_bail!())),
        "cancel" | "unsubscribe" => cmd_cancel(store, args),
        "card" | "cards" => Output::text(digest::by_card(store, args)),
        "cal" | "calendar" | "sync" => cmd_cal(store, args),
        "export" | "backup" => cmd_export(store),
        "import" | "restore" => cmd_import(input),
        "budget" => cmd_budget(store, args),
        "remind" | "reminder" => cmd_remind(store, args),
        "tz" | "timezone" => cmd_tz(store, args),
        "currency" => cmd_currency(store, args),
        "help" | "commands" | "?" => Output::text(digest::help()),
        _ if ai_allowed => cmd_ai_intent(store, verb, args, input, today),
        other => Output::error(format!(
            "I don't know the command `{other}`.\n\n{}",
            digest::help()
        )),
    }
}

// ── Mutating commands ────────────────────────────────────────────────────────

fn cmd_add(store: &mut Store, args: &str, today: Date, ai_allowed: bool) -> Output {
    let spec = match parse::parse_add(args, today) {
        Ok(spec) => spec,
        Err(parse_error) => {
            // The typed form did not parse. If AI is available, let the model
            // read it as prose before giving up, `sub add netflix twenty
            // bucks a month` should still work.
            match ai_allowed.then(|| ai_extract(args, today)).flatten() {
                Some(spec) => spec,
                None => return Output::error(parse_error),
            }
        }
    };
    add_from_spec(store, spec, today)
}

fn cmd_track(
    store: &mut Store,
    args: &str,
    input: &Input,
    today: Date,
    ai_allowed: bool,
) -> Output {
    let source = if !input.text.trim().is_empty() {
        input.text.trim()
    } else {
        args.trim()
    };
    if source.is_empty() {
        return Output::error(
            "Select the receipt or renewal notice first, then run `sub track`. \
             Or add it directly: `sub add Netflix 19.99 monthly`.",
        );
    }
    if !ai_allowed {
        return Output::error("`sub track` needs AI. Try `sub add <name> <price> <cycle>`.");
    }
    match ai_extract(source, today) {
        Some(spec) => add_from_spec(store, spec, today),
        // The selection may be unrelated. Show what is tracked.
        None => Output::text(format!(
            "Nothing subscription-shaped in the selected text.\n\n{}",
            digest::list(store, today)
        )),
    }
}

fn add_from_spec(store: &mut Store, spec: AddSpec, today: Date) -> Output {
    if let Some(existing) = store
        .subs
        .iter()
        .find(|s| s.name.to_lowercase() == spec.name.to_lowercase())
    {
        return Output::error(format!(
            "**{}** is already tracked (id {}). Change it with `sub edit {} price <amount>`, \
             or remove it first.",
            existing.name, existing.id, existing.id
        ));
    }

    // One rule for paid vs trial, applied here. A typed line and an AI
    // extraction reach the same answer. A date with no price is a trial.
    let mut spec = spec;
    if spec.price.is_none() && spec.trial_ends.is_none() {
        spec.trial_ends = spec.next_due.take();
    }

    // A trial's cancel-by date is its first charge date, so it serves as the
    // renewal date too. With no date at all, assume one cycle out.
    let guessed = spec.next_due.is_none() && spec.trial_ends.is_none();
    let due = spec
        .next_due
        .or(spec.trial_ends)
        .unwrap_or_else(|| spec.cycle.advance(today));

    let mut sub = new_sub(spec.name.clone(), spec.price, spec.cycle, due);
    sub.category = spec.category;
    sub.card = spec.card;
    sub.cancel_url = spec.cancel_url;
    sub.trial_ends = spec.trial_ends.map(|d| d.to_string());
    sub.notes = spec.notes;
    sub.reminder_days = spec.reminder_days;

    Store::snapshot_for_undo();
    let id = store.add(sub);
    store.save();

    let sub = &store.subs[store.subs.len() - 1];
    let mut out = match &sub.trial_ends {
        Some(trial) => format!(
            "**{}** trial, cancel by {trial} ({}).",
            sub.name,
            date::phrase_span(today.days_until(due))
        ),
        None => format!(
            "**{}** {} {}, next {} ({}).",
            sub.name,
            store.price_label(sub),
            sub.cycle,
            sub.next_due,
            date::phrase_span(today.days_until(due))
        ),
    };
    if guessed {
        out.push_str(" Date assumed.");
    }
    // `sub show 1` typed into the bar matches no keyword, so it is not offered.
    let _ = id;
    out.push_str("\n\nPress Inspect on the tile for the rest.");
    Output::text(out)
}

fn cmd_trial(store: &mut Store, args: &str, today: Date) -> Output {
    // Naming something already tracked flips it to a trial and keeps its date.
    // `sub edit <name> trial none` flips it back.
    if let Ok(id) = store.resolve(args) {
        let (name, trial) = {
            let sub = store.get_mut(id).expect("resolved id exists");
            let Some(due) = Date::parse(&sub.next_due) else {
                return Output::error(format!(
                    "**{}** has no readable date to cancel by. Set one with \
                     `sub edit {id} trial may 15 2027`.",
                    sub.name
                ));
            };
            sub.trial_ends = Some(due.to_string());
            (sub.name.clone(), due)
        };
        Store::snapshot_for_undo();
        store.save();
        return Output::text(format!(
            "**{name}** is a trial, cancel by {trial} ({}).",
            date::phrase_span(today.days_until(trial))
        ));
    }
    match parse::parse_trial(args, today) {
        Ok(spec) => add_from_spec(store, spec, today),
        Err(e) => Output::error(e),
    }
}

fn cmd_renew(store: &mut Store, args: &str, today: Date) -> Output {
    let id = match store.resolve(args) {
        Ok(id) => id,
        Err(e) => return Output::error(e.to_string()),
    };

    let (name, price, was_paused, previous_price, converted, old_due, new_due, periods) = {
        let sub = store.get_mut(id).expect("resolved id exists");
        let old_due = sub.due_date().unwrap_or(today);
        let (new_due, periods) = sub.cycle.advance_past(old_due, today);
        let previous_price = sub.last_paid().map(|p| p.amount);
        let converted = sub.trial_ends.take().is_some();

        // An amount of zero would make the spend totals wrong. An unpriced
        // subscription rolls its date forward with no payment logged.
        if let Some(amount) = sub.price {
            sub.history.push(Payment { date: today.to_string(), amount });
        }
        sub.next_due = new_due.to_string();
        (
            sub.name.clone(),
            sub.price,
            sub.paused,
            previous_price,
            converted,
            old_due,
            new_due,
            periods,
        )
    };

    Store::snapshot_for_undo();
    store.save();

    let mut out = format!(
        "**{name}** paid. Next {new_due} ({}).",
        date::phrase_span(today.days_until(new_due))
    );
    if periods > 1 {
        out.push_str(&format!(" Skipped {periods} periods from {old_due}."));
    }
    if converted {
        out.push_str(" Trial is over.");
    }
    match (previous_price, price) {
        (Some(prev), Some(now)) if (prev - now).abs() > 0.004 => {
            let direction = if now > prev { "up" } else { "down" };
            out.push_str(&format!(
                "\n\nPrice {direction}: {} to {}.",
                store.money(prev),
                store.money(now)
            ));
        }
        _ => {}
    }
    if was_paused {
        out.push_str(" Still paused.");
    }
    if price.is_none() {
        out.push_str(&format!("\n\nNo amount recorded. `sub edit {id} price 12.99`"));
    }
    Output::text(out)
}

fn cmd_snooze(store: &mut Store, args: &str, today: Date) -> Output {
    let (target, days_text) = parse::split_verb(args);
    // `sub snooze netflix 5` and `sub snooze 5` (id-less, from a tile button
    // that carries only the id) both have to work, so try the whole argument as
    // a target first and only then split a trailing day count off it.
    let (id, days) = match store.resolve(args) {
        Ok(id) => (id, DEFAULT_SNOOZE_DAYS),
        Err(_) => {
            let days = days_text
                .split_whitespace()
                .next()
                .and_then(|d| d.parse::<i64>().ok())
                .unwrap_or(DEFAULT_SNOOZE_DAYS);
            match store.resolve(&target) {
                Ok(id) => (id, days),
                Err(e) => return Output::error(e.to_string()),
            }
        }
    };
    if days <= 0 {
        return Output::error("Snooze needs a positive number of days, e.g. `sub snooze netflix 7`.");
    }

    let (name, new_due) = {
        let sub = store.get_mut(id).expect("resolved id exists");
        // Push from today when the date has already passed, so snoozing an
        // overdue item actually moves it into the future.
        let base = sub.due_date().filter(|d| *d > today).unwrap_or(today);
        let new_due = base.add_days(days);
        sub.next_due = new_due.to_string();
        (sub.name.clone(), new_due)
    };

    Store::snapshot_for_undo();
    store.save();
    Output::text(format!(
        "**{name}** moved to {new_due} ({}). Nothing recorded as paid.",
        date::phrase_span(today.days_until(new_due))
    ))
}

fn cmd_pause(store: &mut Store, args: &str, paused: bool) -> Output {
    let id = match store.resolve(args) {
        Ok(id) => id,
        Err(e) => return Output::error(e.to_string()),
    };
    let (name, changed) = {
        let sub = store.get_mut(id).expect("resolved id exists");
        let changed = sub.paused != paused;
        sub.paused = paused;
        (sub.name.clone(), changed)
    };
    if !changed {
        return Output::text(format!(
            "**{name}** is already {}.",
            if paused { "paused" } else { "active" }
        ));
    }
    Store::snapshot_for_undo();
    store.save();
    Output::text(format!(
        "**{name}** {}.",
        if paused { "paused, off the tile and out of totals" } else { "active again" }
    ))
}

fn cmd_edit(store: &mut Store, args: &str, today: Date) -> Output {
    let (target, field, value) = match parse::parse_edit(args) {
        Ok(parts) => parts,
        Err(e) => return Output::error(e),
    };
    let id = match store.resolve(&target) {
        Ok(id) => id,
        Err(e) => return Output::error(e.to_string()),
    };
    let clearing = matches!(value.trim().to_lowercase().as_str(), "none" | "clear" | "-");

    let currency = store.currency.clone();
    let sub = store.get_mut(id).expect("resolved id exists");
    let name = sub.name.clone();
    let described = match field.as_str() {
        "price" | "cost" if clearing => {
            sub.price = None;
            "price cleared".to_string()
        }
        "price" | "cost" => match parse::parse_money(&value) {
            Some(price) => {
                sub.price = Some(price);
                format!("price is now {currency}{price:.2}")
            }
            None => return Output::error(format!("\"{value}\" isn't a price.")),
        },
        "cycle" => match Cycle::parse(&value) {
            Some(cycle) => {
                sub.cycle = cycle;
                format!("billing cycle is now {cycle}")
            }
            None => {
                return Output::error(format!(
                    "I don't understand the cycle \"{value}\". Try monthly, yearly, weekly, quarterly, or `45 days`."
                ));
            }
        },
        "next" | "due" => match parse::parse_when(&value, today) {
            Some(date) => {
                sub.next_due = date.to_string();
                format!("next renewal is {date}, {}", date::phrase_due(today.days_until(date)))
            }
            None => {
                return Output::error(format!(
                    "I don't understand the date \"{value}\". Use YYYY-MM-DD, `tomorrow`, or `+10d`."
                ));
            }
        },
        "trial" if clearing => {
            sub.trial_ends = None;
            "no longer marked as a trial".to_string()
        }
        "trial" => match parse::parse_when(&value, today) {
            Some(date) => {
                sub.trial_ends = Some(date.to_string());
                format!("free trial, cancel by {date}")
            }
            None => return Output::error(format!("I don't understand the date \"{value}\".")),
        },
        "name" => {
            sub.name = value.trim().to_string();
            format!("renamed to {}", sub.name)
        }
        "cat" | "category" => {
            sub.category = (!clearing).then(|| value.trim().to_string());
            if clearing { "category cleared".into() } else { format!("category is {value}") }
        }
        "card" => {
            sub.card = (!clearing).then(|| value.trim().trim_start_matches('*').to_string());
            if clearing { "card cleared".into() } else { format!("card: {value}") }
        }
        "url" | "cancel" => {
            sub.cancel_url = (!clearing).then(|| value.trim().to_string());
            if clearing { "cancel link cleared".into() } else { "cancel link saved".into() }
        }
        "note" | "notes" => {
            sub.notes = (!clearing).then(|| value.trim().to_string());
            if clearing { "note cleared".into() } else { format!("note: {value}") }
        }
        // There is no delete API for calendar events, so clearing the stored id
        // is how a user tells us to build a fresh one after changing the date.
        "cal" | "calendar" if clearing => {
            let had = sub.calendar_event_id.take().is_some();
            if had {
                "calendar link cleared, `sub cal` will create a new event (delete the old one in Google Calendar)".to_string()
            } else {
                "no calendar event was linked".to_string()
            }
        }
        "cal" | "calendar" => {
            return Output::error(
                "Create calendar events with `sub cal <name>`. `sub edit <name> cal none` \
                 unlinks the existing one so a fresh event can be made.",
            );
        }
        "remind" => {
            if clearing {
                sub.reminder_days = None;
                "reminder back to the default".to_string()
            } else {
                match value.trim().parse::<u32>() {
                    Ok(days) => {
                        sub.reminder_days = Some(days);
                        format!("reminder {days} days before renewal")
                    }
                    Err(_) => {
                        return Output::error(format!("\"{value}\" isn't a number of days."));
                    }
                }
            }
        }
        other => return Output::error(format!("Can't edit `{other}`.")),
    };

    Store::snapshot_for_undo();
    store.save();
    Output::text(format!("**{name}**: {described}."))
}

fn cmd_remove(store: &mut Store, args: &str) -> Output {
    let id = match store.resolve(args) {
        Ok(id) => id,
        Err(e) => return Output::error(e.to_string()),
    };
    Store::snapshot_for_undo();
    let removed = store.remove(id).expect("resolved id exists");
    store.save();
    Output::text(format!(
        "Removed **{}**. `sub undo` to bring it back.",
        removed.name
    ))
}

fn cmd_undo() -> Output {
    if Store::undo() {
        let store = Store::load();
        Output::text(format!(
            "Undone. {} tracked. `sub undo` again to redo.",
            plural(store.subs.len(), "subscription")
        ))
    } else {
        Output::error("Nothing to undo.")
    }
}

fn cmd_show(store: &Store, args: &str, today: Date) -> Output {
    match store.resolve(args) {
        Ok(id) => {
            let sub = store.subs.iter().find(|s| s.id == id).expect("resolved");
            Output::text(digest::detail(store, sub, today))
        }
        Err(e) => Output::error(e.to_string()),
    }
}

fn cmd_cancel(store: &Store, args: &str) -> Output {
    let id = match store.resolve(args) {
        Ok(id) => id,
        Err(e) => return Output::error(e.to_string()),
    };
    let sub = store.subs.iter().find(|s| s.id == id).expect("resolved");
    match &sub.cancel_url {
        // The bare URL is the whole result on purpose: the popup's ⌘C copies
        // the entire result body, and `Output::copy` is discarded for community
        // extensions (commands/community.rs).
        Some(url) => Output::text(url.clone()),
        None => Output::error(format!(
            "No cancellation link saved for **{}**. Add one with \
             `sub edit {} url https://…`, then `sub cancel {}` will hand it back for ⌘C.",
            sub.name, sub.id, sub.id
        )),
    }
}

fn cmd_cal(store: &mut Store, args: &str) -> Output {
    let targets: Vec<u32> = if matches!(args.trim().to_lowercase().as_str(), "all" | "") {
        store.active().map(|s| s.id).collect()
    } else {
        match store.resolve(args) {
            Ok(id) => vec![id],
            Err(e) => return Output::error(e.to_string()),
        }
    };
    if targets.is_empty() {
        return Output::error("Nothing active to put on the calendar.");
    }

    let mut created = Vec::new();
    let mut skipped = Vec::new();
    let mut failed = Vec::new();

    for id in targets {
        let snapshot = store.subs.iter().find(|s| s.id == id).cloned();
        let Some(sub) = snapshot else { continue };
        if sub.calendar_event_id.is_some() {
            skipped.push(sub.name.clone());
            continue;
        }
        let Some(due) = sub.trial_date().or_else(|| sub.due_date()) else {
            failed.push(format!("{} (unreadable date)", sub.name));
            continue;
        };
        let reminder = store.reminder_days_for(&sub).min(MAX_REMINDER_DAYS);
        match create_calendar_event(store, &sub, due, reminder) {
            Ok(event_id) => {
                if let Some(target) = store.get_mut(id) {
                    target.calendar_event_id = Some(event_id);
                }
                created.push(sub.name.clone());
            }
            Err(e) => failed.push(format!("{} ({e})", sub.name)),
        }
    }

    if !created.is_empty() {
        Store::snapshot_for_undo();
        store.save();
    }

    let mut out = String::new();
    if !created.is_empty() {
        out.push_str(&format!(
            "## Calendar reminders created\n{}\n\nTrials get one event on the cancel-by date. \
             Everything else gets a recurring event on the renewal date. Both notify you \
             {} days ahead.\n",
            created
                .iter()
                .map(|n| format!("- **{n}**"))
                .collect::<Vec<_>>()
                .join("\n"),
            store.default_reminder_days
        ));
    }
    if !skipped.is_empty() {
        out.push_str(&format!(
            "Already on the calendar: {}. To rebuild one, run `sub edit <name> cal none` \
             and delete the old event in Google Calendar first.\n",
            skipped.join(", ")
        ));
    }
    if !failed.is_empty() {
        out.push_str(&format!("\nCouldn't create: {}.\n", failed.join(", ")));
    }
    let out = out.trim().to_string();
    if out.is_empty() {
        Output::error("Nothing to do.")
    } else if created.is_empty() && !failed.is_empty() {
        Output::error(out)
    } else {
        Output::text(out)
    }
}

/// Create the reminder event. The host forwards this payload verbatim to
/// Google's API (`pointiv-backend/src/routes/google.rs`), so the recurrence
/// rule and the explicit reminder override both take effect.
///
/// A free trial gets one event on its cancel-by date. Everything else gets a
/// recurring event that covers future renewals.
fn create_calendar_event(
    store: &Store,
    sub: &Sub,
    due: Date,
    reminder_days: u32,
) -> Result<String, String> {
    let trial = sub.trial_date();
    let date = trial.unwrap_or(due);

    let summary = match (trial, sub.price) {
        (Some(_), _) => format!("Cancel {} before it charges you", sub.name),
        (None, Some(price)) => format!("{} renews, {}", sub.name, store.money(price)),
        (None, None) => format!("{} renews", sub.name),
    };

    let mut description = match sub.price {
        Some(price) => format!("{} {}", store.money(price), sub.cycle),
        None => format!("{}, price not recorded", sub.cycle),
    };
    description.push_str(". Tracked by Pointiv Subscription Manager.");
    if let Some(card) = &sub.card {
        description.push_str(&format!("\n\nCard: {card}"));
    }
    if let Some(url) = &sub.cancel_url {
        description.push_str(&format!("\n\nCancel: {url}"));
    }

    let mut payload = serde_json::json!({
        "summary": summary,
        "description": description,
        // All-day events take an exclusive end date.
        "start": { "date": date.to_string() },
        "end": { "date": date.add_days(1).to_string() },
        "reminders": {
            "useDefault": false,
            "overrides": [{ "method": "popup", "minutes": reminder_days * 24 * 60 }]
        }
    });
    if trial.is_none() {
        payload["recurrence"] = serde_json::json!([sub.cycle.rrule()]);
    }

    let response = google_calendar::create_event_raw(&payload)?;
    Ok(response["id"].as_str().unwrap_or("created").to_string())
}

fn cmd_export(store: &Store) -> Output {
    // Bare JSON, so ⌘C in the result panel copies a usable backup.
    match serde_json::to_string_pretty(store) {
        Ok(json) => Output::text(json),
        Err(e) => Output::error(format!("Couldn't serialise the store: {e}")),
    }
}

fn cmd_import(input: &Input) -> Output {
    let raw = input.text.trim();
    if raw.is_empty() {
        return Output::error(
            "Select the JSON from `sub export` first, then run `sub import`. \
             This replaces everything currently tracked (`sub undo` reverts it).",
        );
    }
    let incoming: Store = match serde_json::from_str(raw) {
        Ok(store) => store,
        Err(e) => return Output::error(format!("That isn't a valid export: {e}")),
    };
    Store::snapshot_for_undo();
    // Keep ids collision-free even if the document was hand-edited.
    let mut store = incoming;
    let max_id = store.subs.iter().map(|s| s.id).max().unwrap_or(0);
    store.next_id = store.next_id.max(max_id + 1);
    store.version = model::SCHEMA_VERSION;
    store.save();
    Output::text(format!(
        "Imported {} subscriptions, replacing what was here.\n\n{}\n\nRevert with `sub undo`.",
        store.subs.len(),
        digest::totals_line(&store)
    ))
}

fn cmd_budget(store: &mut Store, args: &str) -> Output {
    let value = args.trim();
    if value.is_empty() {
        return match store.budget_monthly {
            Some(budget) => Output::text(format!(
                "Monthly budget is {}. Current spend {}.\n\nChange it with `sub budget 75`, \
                 clear it with `sub budget none`.",
                store.money(budget),
                store.money(store.monthly_total())
            )),
            None => Output::text("No budget set. Try `sub budget 75`.".to_string()),
        };
    }
    if matches!(value.to_lowercase().as_str(), "none" | "clear" | "off") {
        store.budget_monthly = None;
        store.save();
        return Output::text("Budget cleared.".to_string());
    }
    match parse::parse_money(value) {
        Some(budget) => {
            store.budget_monthly = Some(budget);
            store.save();
            let monthly = store.monthly_total();
            Output::text(format!(
                "Monthly budget set to {}. You're at {}, {}.",
                store.money(budget),
                store.money(monthly),
                if monthly > budget {
                    format!("**over by {}**", store.money(monthly - budget))
                } else {
                    format!("{} to spare", store.money(budget - monthly))
                }
            ))
        }
        None => Output::error(format!("\"{value}\" isn't an amount. Try `sub budget 75`.")),
    }
}

fn cmd_remind(store: &mut Store, args: &str) -> Output {
    let value = args.trim();
    if value.is_empty() {
        return Output::text(format!(
            "Calendar reminders fire {} days before a renewal. Change it with `sub remind 5`.",
            store.default_reminder_days
        ));
    }
    match value.parse::<u32>() {
        Ok(days) if days <= MAX_REMINDER_DAYS => {
            store.default_reminder_days = days;
            store.save();
            Output::text(format!(
                "New calendar reminders will fire {days} days before renewal. \
                 Existing events keep their original timing."
            ))
        }
        Ok(_) => Output::error(format!(
            "Google caps reminders at {MAX_REMINDER_DAYS} days before an event."
        )),
        Err(_) => Output::error(format!("\"{value}\" isn't a number of days.")),
    }
}

fn cmd_tz(store: &mut Store, args: &str) -> Output {
    let value = args.trim();
    if value.is_empty() {
        let hours = store.tz_offset_minutes as f64 / 60.0;
        return Output::text(format!(
            "Dates use UTC{hours:+}. Set yours with `sub tz -7` so \"today\" matches your day."
        ));
    }
    let hours: f64 = match value.trim_start_matches("UTC").trim_start_matches("utc").parse() {
        Ok(h) => h,
        Err(_) => return Output::error(format!("\"{value}\" isn't an hour offset. Try `sub tz -7`.")),
    };
    if !(-14.0..=14.0).contains(&hours) {
        return Output::error("Offsets run from -14 to +14 hours.");
    }
    store.tz_offset_minutes = (hours * 60.0).round() as i32;
    store.save();
    match resolve_today(store) {
        Some(today) => Output::text(format!("Offset set to UTC{hours:+}. Today is {today}.")),
        None => Output::text(format!("Offset set to UTC{hours:+}.")),
    }
}

fn cmd_currency(store: &mut Store, args: &str) -> Output {
    let symbol = args.trim();
    if symbol.is_empty() {
        return Output::text(format!(
            "Amounts show as {}. Change it with `sub currency £`.",
            store.money(9.99)
        ));
    }
    if symbol.chars().count() > 4 {
        return Output::error("Use a short symbol or code, e.g. `$`, `£`, `CHF`.");
    }
    store.currency = symbol.to_string();
    store.save();
    Output::text(format!("Amounts now show as {}.", store.money(9.99)))
}

// ── AI paths ─────────────────────────────────────────────────────────────────

/// Ask the model to pull a subscription out of arbitrary text.
fn ai_extract(source: &str, today: Date) -> Option<AddSpec> {
    let clipped: String = source.chars().take(4000).collect();
    let prompt = format!(
        "Extract the subscription described below. Reply with ONLY a JSON object, no prose, \
         no code fence, using exactly these keys:\n\
         {{\"name\":string,\"price\":number,\"cycle\":\"weekly\"|\"monthly\"|\"quarterly\"|\"yearly\",\
         \"next_due\":\"YYYY-MM-DD\"|null,\"trial_ends\":\"YYYY-MM-DD\"|null,\
         \"category\":string|null,\"cancel_url\":string|null,\"card\":string|null}}\n\
         Rules: price is the recurring charge as a plain number with no currency symbol. \
         name is the service, not the plan tier. Today is {today}; resolve relative dates \
         against it and never return a next_due in the past. Use null when the text does not say.\n\n\
         TEXT:\n{clipped}"
    );
    let reply = ai::complete(&prompt);
    if reply.trim().is_empty() {
        log::warn("ai_extract: empty reply (no ai permission, or not signed in)");
        return None;
    }
    match AddSpec::from_json(&reply, today) {
        Ok(spec) => Some(spec),
        Err(e) => {
            log::warn(&format!("ai_extract: {e}"));
            None
        }
    }
}

/// Map a sentence to one of the commands above and run it on the normal path.
/// The reply echoes the interpretation, so a misread is visible.
fn cmd_ai_intent(
    store: &mut Store,
    verb: &str,
    args: &str,
    input: &Input,
    today: Option<Date>,
) -> Output {
    let request = format!("{verb} {args}").trim().to_string();
    let names: Vec<String> = store
        .subs
        .iter()
        .map(|s| format!("{} (id {})", s.name, s.id))
        .collect();
    let prompt = format!(
        "You translate a request into exactly one command for a subscription tracker. \
         Reply with ONLY the command line, no explanation, no backticks.\n\n\
         Commands:\n\
         subs | sub add <name> <price> <cycle> [next <date>] [cat <x>] [card <x>] [url <x>] [trial <date>]\n\
         sub renew <name> | sub snooze <name> <days> | sub pause <name> | sub resume <name>\n\
         sub edit <name> <price|cycle|next|cat|card|url|trial|note|name> <value>\n\
         sub remove <name> | sub show <name> | sub spend | sub trials | sub cancel <name>\n\
         sub card <digits> | sub cal <name|all> | sub export | sub budget <amount> | sub help\n\n\
         Tracked: {}\n\
         Today: {}\n\n\
         Request: {request}",
        if names.is_empty() { "nothing yet".to_string() } else { names.join(", ") },
        today.map(|t| t.to_string()).unwrap_or_else(|| "unknown".into())
    );

    let reply = ai::complete(&prompt);
    let line = reply
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .trim_matches('`')
        .trim();
    if line.is_empty() {
        return Output::error(format!(
            "I don't know the command `{verb}`, and AI isn't available to interpret it \
             (sign in under Settings → Account, or grant the `ai` permission).\n\n{}",
            digest::help()
        ));
    }

    let (next_verb, next_args) = parse::split_verb(strip_prefix(line));
    if next_verb == verb || next_verb.is_empty() && args.is_empty() {
        // The model echoed the request back, or produced nothing runnable.
        return Output::text(digest::list(store, match today {
            Some(t) => t,
            None => return Output::error(NO_DATE),
        }));
    }

    log::info(&format!("ai_intent: {request:?} -> {line:?}"));
    // ai_allowed = false: one translation per invocation, no recursion.
    let result = dispatch(store, &next_verb, next_args, input, today, false);
    // A bare-URL result (sub cancel) must stay bare for ⌘C, so only annotate
    // text results that are already prose.
    if result.kind == "text" && result.value.contains(' ') {
        Output::text(format!("{}\n\n_Read as `{line}`._", result.value))
    } else {
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_converts_to_a_calendar_date() {
        // 2026-09-17T19:30:00Z
        assert_eq!(date_from_epoch_secs(1_789_673_400, 0).unwrap().to_string(), "2026-09-17");
        // 2026-09-17T00:00:00Z, exactly the day boundary.
        assert_eq!(date_from_epoch_secs(1_789_603_200, 0).unwrap().to_string(), "2026-09-17");
    }

    #[test]
    fn timezone_offset_shifts_the_day_boundary() {
        // 2026-09-17T23:30Z is still the 17th at UTC-7 and already the 18th at UTC+2.
        let secs = 1_789_687_800;
        assert_eq!(date_from_epoch_secs(secs, 0).unwrap().to_string(), "2026-09-17");
        assert_eq!(date_from_epoch_secs(secs, -7 * 60).unwrap().to_string(), "2026-09-17");
        assert_eq!(date_from_epoch_secs(secs, 2 * 60).unwrap().to_string(), "2026-09-18");
    }

    #[test]
    fn the_observed_probe_timestamp_needs_the_offset_to_read_correctly() {
        // The value the real host returned during the clock probe. In UTC it is
        // already the 18th while the machine's own day was still the 17th,
        // which is exactly why `sub tz` exists.
        let probe = 1_789_713_367;
        assert_eq!(date_from_epoch_secs(probe, 0).unwrap().to_string(), "2026-09-18");
        assert_eq!(date_from_epoch_secs(probe, -7 * 60).unwrap().to_string(), "2026-09-17");
    }

    #[test]
    fn implausible_clock_values_are_rejected() {
        assert!(date_from_epoch_secs(0, 0).is_none(), "1970 means a broken clock");
        assert!(date_from_epoch_secs(-1, 0).is_none());
        assert!(date_from_epoch_secs(i64::MAX, 0).is_none());
        assert!(date_from_epoch_secs(1_600_000_000, 0).is_none(), "2020 predates the extension");
    }

    #[test]
    fn tile_now_shifts_by_the_wall_clock_hour() {
        let mut store = Store::default();
        assert_eq!(tz_day_shift(&store, "2026-09-17T23:30:00Z"), 0, "no offset, no shift");
        store.tz_offset_minutes = -7 * 60;
        assert_eq!(tz_day_shift(&store, "2026-09-17T23:30:00Z"), 0, "16:30 local, same day");
        assert_eq!(tz_day_shift(&store, "2026-09-17T03:00:00Z"), -1, "20:00 the previous day");
        store.tz_offset_minutes = 2 * 60;
        assert_eq!(tz_day_shift(&store, "2026-09-17T23:30:00Z"), 1, "01:30 the next day");
        // No readable time means midnight UTC, which never shifts the day.
        assert_eq!(tz_day_shift(&store, "garbage"), 0);
    }

    #[test]
    fn last_seen_is_written_only_when_the_date_moves() {
        let mut store = Store::default();
        let today = Date::parse("2026-09-17").unwrap();
        assert!(touch_last_seen(&mut store, today), "first observation writes");
        assert!(!touch_last_seen(&mut store, today), "same day does not");
        assert!(touch_last_seen(&mut store, today.add_days(1)), "a new day does");
    }

    #[test]
    fn a_dead_clock_falls_back_to_the_stored_date() {
        let store = Store {
            last_seen: Some("2026-09-17".into()),
            ..Store::default()
        };
        let clock = Date::parse("2026-09-20");
        assert_eq!(today_from(clock, &store), clock, "a working clock wins");
        assert_eq!(
            today_from(None, &store),
            Date::parse("2026-09-17"),
            "a dead clock falls back to the last date seen"
        );
        assert_eq!(today_from(None, &Store::default()), None, "nothing to fall back to");
        let corrupt = Store { last_seen: Some("whenever".into()), ..Store::default() };
        assert_eq!(today_from(None, &corrupt), None, "an unparseable stamp is not a date");
    }

    #[test]
    fn the_real_clock_answers_in_this_sandbox() {
        // The probe established that Extism's WASI serves clock_time_get; this
        // keeps that assumption honest on the host target too.
        assert!(clock_date(0).is_some());
    }

    #[test]
    fn successful_output_is_wrapped_so_a_tile_button_shows_it() {
        // The popup drops a tile action's plain success, so Inspect would be a
        // dead button without the sentinel.
        let wrapped = show(Output::text("## AMC\n- details"));
        assert_eq!(wrapped.kind, "text");
        assert_eq!(wrapped.value, "__OVERLAY__\n## AMC\n- details");

        // Errors already reach the popup on their own.
        let err = show(Output::error("nope"));
        assert_eq!(err.value, "nope");

        // An existing sentinel stays as it is.
        let already = show(Output::text("__CONFIRM__\nhi"));
        assert_eq!(already.value, "__CONFIRM__\nhi");
    }

    #[test]
    fn the_sub_prefix_is_optional_and_case_insensitive() {
        assert_eq!(strip_prefix("sub renew netflix"), "renew netflix");
        assert_eq!(strip_prefix("SUB RENEW netflix"), "RENEW netflix");
        assert_eq!(strip_prefix("subs"), "");
        assert_eq!(strip_prefix("sub"), "");
        assert_eq!(strip_prefix("subscriptions"), "");
        assert_eq!(strip_prefix("subscription add Netflix 19.99"), "add Netflix 19.99");
        assert_eq!(strip_prefix("renew netflix"), "renew netflix", "bare verbs pass through");
        assert_eq!(strip_prefix("submarine service 9.99"), "submarine service 9.99",
                   "a word that merely starts with sub is left alone");
        assert_eq!(strip_prefix(""), "");
    }

    #[test]
    fn tile_action_commands_round_trip_through_the_dispatcher() {
        // Every command the tile can mint must parse back to a known verb.
        for command in [
            "sub renew 3",
            "sub snooze 3",
            "sub cancel 3",
            "sub edit 3 trial none",
            "sub add",
            "subs",
            "sub spend",
            "sub show 3",
        ] {
            let (verb, _args) = parse::split_verb(strip_prefix(command));
            assert!(
                matches!(
                    verb.as_str(),
                    "" | "renew" | "snooze" | "cancel" | "edit" | "add" | "spend" | "show"
                ),
                "tile command {command:?} produced unknown verb {verb:?}"
            );
        }
    }
}
