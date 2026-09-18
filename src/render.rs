//! The popup tile.
//!
//! `render_tile` gets storage access and a 3 second budget. This module
//! formats the stored document and the `now` the host passes in.
//!
//! A tile card is 300px wide. A row renders as one horizontal line: `text`
//! (flex, ellipsis), `secondary`, the badge chip, then the buttons, all
//! `nowrap` (`@katkode/pointiv-ui/dist/backends/mui/index.js`). A short label,
//! a small badge, and one button fit that space. Anything more overflows
//! sideways. Rows here carry a label, a badge, and one button.
//!
//! `TileNode::Countdown` is unused. Its renderer prints "now" for every past
//! deadline, which drops the overdue count. This module computes the relative
//! phrasing.

use crate::date::Date;
use crate::digest::DUE_SOON_DAYS;
use crate::model::{DeadlineKind, Store, Sub};
use pointiv_extension_sdk::prelude::*;

/// Rows shown at once. Four lines of ~38px fit the 276px card at height 2.
const MAX_ROWS: usize = 4;
/// Row text is ellipsised by the renderer, so this only has to stop a
/// pathological name from bloating the JSON.
const MAX_LABEL: usize = 34;

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let kept: String = text.chars().take(max.saturating_sub(1)).collect();
    format!("{kept}…")
}

fn tone_for(days: i64) -> TileTone {
    match days {
        d if d < 0 => TileTone::Danger,
        d if d <= DUE_SOON_DAYS => TileTone::Warn,
        _ => TileTone::Neutral,
    }
}

/// Build the tile. `today` comes from the host's `now`. With no date, the tile
/// says so and shows no timings.
pub fn tile(store: &Store, today: Option<Date>) -> TileUi {
    let Some(today) = today else {
        return TileUi::new("Subscriptions")
            .text_toned("Cannot read today's date.", TileTone::Danger, false)
            .footer("All", "subs");
    };

    if store.subs.is_empty() {
        return TileUi::new("Subscriptions")
            .component(
                "mui-alert",
                serde_json::json!({
                    "text": "Nothing tracked yet. Add one below.",
                    "severity": "info"
                }),
            )
            .footer_input("Add", "sub add", "Netflix 19.99 monthly");
    }

    // Ordered by the deadline that matters, most urgent first. The `subs`
    // digest uses the same ordering, so the two views agree.
    let rows = store.by_due(today);
    let due = rows
        .iter()
        .filter(|(_, d)| d.is_some_and(|(days, _)| days <= DUE_SOON_DAYS))
        .count();

    let mut tile = TileUi::new("Subscriptions").subtitle(truncate(&subtitle(store, due), 120));

    for (sub, deadline) in rows.iter().take(MAX_ROWS) {
        tile = tile.row(row(sub, *deadline));
    }
    if rows.len() > MAX_ROWS {
        tile = tile.text_toned(
            format!("+{} more", rows.len() - MAX_ROWS),
            TileTone::Neutral,
            true,
        );
    }

    tile.footer_input("Add", "sub add", "Netflix 19.99 monthly")
        .footer("All", "subs")
}

fn subtitle(store: &Store, due: usize) -> String {
    let mut line = format!("{}/mo", store.money(store.monthly_total()));
    if due > 0 {
        line.push_str(&format!(" · {due} due"));
    }
    let unpriced = store.unpriced();
    if unpriced > 0 {
        line.push_str(&format!(" · {unpriced} no price"));
    }
    line
}

/// One row: the name, a countdown, and Inspect. Inspect opens `sub show`,
/// which carries everything else.
fn row(sub: &Sub, deadline: Option<(i64, DeadlineKind)>) -> RowBuilder {
    let builder = RowBuilder::new(truncate(&sub.name, MAX_LABEL));
    let inspect = format!("sub show {}", sub.id);

    match deadline {
        Some((days, _)) => builder.badge(countdown(days), tone_for(days)).action("Inspect", inspect),
        None => builder.badge("date?", TileTone::Danger).action("Inspect", inspect),
    }
}

/// Short enough to sit beside a name and a button in a 300px card. An overdue
/// item counts up: "3d ago", "12d ago".
fn countdown(days: i64) -> String {
    match days {
        d if d < 0 => format!("{}d ago", -d),
        0 => "today".to_string(),
        1 => "tomorrow".to_string(),
        d => format!("{d}d"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::date::Cycle;
    use crate::model::new_sub;

    // Limits the host validator enforces. Mirrored here so a regression fails
    // in the test suite. In the popup it would blank the tile.
    const MAX_NODES: usize = 16;
    const MAX_TITLE: usize = 80;
    const MAX_SUBTITLE: usize = 120;
    const MAX_TEXT: usize = 300;
    const MAX_BADGE: usize = 40;
    const MAX_ACTION_LABEL: usize = 40;
    const MAX_ACTION_COMMAND: usize = 512;
    const MAX_ROW_ACTIONS: usize = 2;
    const MAX_FOOTER_ACTIONS: usize = 3;

    fn date(s: &str) -> Date {
        Date::parse(s).unwrap()
    }

    fn today() -> Option<Date> {
        Some(date("2026-09-17"))
    }

    fn sample() -> Store {
        let mut store = Store::default();
        store.add(new_sub("Netflix".into(), Some(19.99), Cycle::MONTHLY, date("2026-09-14")));
        store.add(new_sub("Spotify".into(), Some(11.99), Cycle::MONTHLY, date("2026-09-19")));
        store.add(new_sub("Figma".into(), Some(144.0), Cycle::YEARLY, date("2027-01-10")));
        store
    }

    /// Walk the tile the way the host validator does.
    fn check_limits(tile: &TileUi) {
        assert!(tile.title.chars().count() <= MAX_TITLE, "title too long");
        assert!(!tile.title.trim().is_empty(), "title must be non-empty");
        if let Some(sub) = &tile.subtitle {
            assert!(sub.chars().count() <= MAX_SUBTITLE, "subtitle too long: {sub}");
            assert!(!sub.trim().is_empty());
        }
        assert!(tile.footer.len() <= MAX_FOOTER_ACTIONS, "too many footer actions");
        for action in &tile.footer {
            check_action(action);
        }
        let mut nodes = 0;
        for node in &tile.body {
            count_node(node, &mut nodes, 1);
        }
        assert!(nodes <= MAX_NODES, "{nodes} nodes exceeds the cap");
    }

    fn check_action(action: &TileAction) {
        assert!(action.label.chars().count() <= MAX_ACTION_LABEL, "label: {}", action.label);
        assert!(!action.label.trim().is_empty());
        assert!(action.command.chars().count() <= MAX_ACTION_COMMAND);
        assert!(!action.command.trim().is_empty());
        if let Some(placeholder) = &action.input {
            assert!(placeholder.chars().count() <= 60, "placeholder: {placeholder}");
        }
    }

    fn count_node(node: &TileNode, nodes: &mut usize, depth: usize) {
        *nodes += 1;
        assert!(depth <= 3, "nesting deeper than 3");
        match node {
            TileNode::Text { text, .. } => {
                assert!(text.chars().count() <= MAX_TEXT, "text: {text}");
                assert!(!text.trim().is_empty());
            }
            TileNode::Row { text, secondary, badge, actions } => {
                assert!(text.chars().count() <= MAX_TEXT, "row text: {text}");
                assert!(!text.trim().is_empty());
                if let Some(s) = secondary {
                    assert!(s.chars().count() <= MAX_TEXT, "secondary: {s}");
                    assert!(!s.trim().is_empty());
                }
                if let Some(b) = badge {
                    assert!(b.label.chars().count() <= MAX_BADGE, "badge: {}", b.label);
                    assert!(!b.label.trim().is_empty());
                }
                assert!(actions.len() <= MAX_ROW_ACTIONS, "too many row actions");
                for action in actions {
                    check_action(action);
                }
            }
            TileNode::Badge { label, .. } => {
                assert!(label.chars().count() <= MAX_BADGE, "badge: {label}")
            }
            TileNode::Progress { value, .. } => {
                assert!((0.0..=1.0).contains(value), "progress out of range")
            }
            TileNode::Countdown { .. } => panic!("countdown renders past deadlines as \"now\""),
            TileNode::Divider => {}
            TileNode::Component { children, .. } => {
                for child in children {
                    count_node(child, nodes, depth + 1);
                }
            }
        }
    }

    /// A row's fields, flattened for assertions.
    struct Row<'a> {
        text: &'a str,
        secondary: Option<&'a str>,
        badge: Option<&'a TileBadge>,
        actions: &'a [TileAction],
    }

    fn rows_of(tile: &TileUi) -> Vec<Row<'_>> {
        tile.body
            .iter()
            .filter_map(|n| match n {
                TileNode::Row { text, secondary, badge, actions } => Some(Row {
                    text,
                    secondary: secondary.as_deref(),
                    badge: badge.as_ref(),
                    actions,
                }),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn overdue_row_says_how_long_ago_and_offers_renew() {
        let tile = tile(&sample(), today());
        check_limits(&tile);
        let rows = rows_of(&tile);
        assert_eq!(rows[0].text, "Netflix", "the name alone");
        assert_eq!(rows[0].badge.unwrap().label, "3d ago");
        assert_eq!(rows[0].badge.unwrap().tone, Some(TileTone::Danger));
        assert_eq!(rows[0].actions[0].label, "Inspect");
        assert_eq!(rows[0].actions[0].command, "sub show 1");
    }

    /// A row renders as one horizontal line inside a 300px card. These are the
    /// budgets that keep it from overflowing sideways.
    #[test]
    fn rows_stay_narrow_enough_for_the_card() {
        let mut store = sample();
        store.add(new_sub("Audible".into(), Some(14.95), Cycle::MONTHLY, date("2026-10-17")));
        store.get_mut(4).unwrap().trial_ends = Some("2026-09-19".into());
        let tile = tile(&store, today());
        check_limits(&tile);
        for row in rows_of(&tile) {
            assert!(row.secondary.is_none(), "a secondary pushes the row over: {}", row.text);
            assert_eq!(row.actions.len(), 1, "one button per row: {}", row.text);
            assert!(row.text.chars().count() <= MAX_LABEL, "label: {}", row.text);
            assert!(
                row.badge.unwrap().label.chars().count() <= 10,
                "badge: {}",
                row.badge.unwrap().label
            );
            assert!(row.actions[0].label.chars().count() <= 7, "button: {}", row.actions[0].label);
        }
        assert!(tile.subtitle.clone().unwrap().chars().count() <= 34, "subtitle must be short");
        assert_eq!(tile.footer.len(), 2, "two footer controls");
    }

    #[test]
    fn rows_run_most_urgent_first() {
        let tile = tile(&sample(), today());
        let names: Vec<&str> = rows_of(&tile).iter().map(|r| r.text).collect();
        assert_eq!(names[0], "Netflix", "overdue first");
        assert!(names[1].starts_with("Spotify"), "then due soon");
        assert!(names[2].starts_with("Figma"));
    }

    #[test]
    fn every_row_is_the_same_shape() {
        let mut store = sample();
        store.add(new_sub("Audible".into(), None, Cycle::MONTHLY, date("2026-10-17")));
        store.get_mut(4).unwrap().trial_ends = Some("2026-09-19".into());
        for row in rows_of(&tile(&store, today())) {
            assert!(!row.text.contains('$'), "no price on the row: {}", row.text);
            assert_eq!(row.actions.len(), 1);
            assert_eq!(row.actions[0].label, "Inspect");
        }
    }

    #[test]
    fn due_soon_and_upcoming_tones_differ() {
        let tile = tile(&sample(), today());
        let rows = rows_of(&tile);
        assert_eq!(rows[1].badge.unwrap().label, "2d");
        assert_eq!(rows[1].badge.unwrap().tone, Some(TileTone::Warn));
        assert_eq!(rows[2].badge.unwrap().tone, Some(TileTone::Neutral));
    }

    #[test]
    fn commands_carry_stable_ids_not_positions() {
        let mut store = sample();
        store.remove(1); // drop Netflix; Spotify keeps id 2
        let tile = tile(&store, today());
        let rows = rows_of(&tile);
        assert!(rows[0].text.starts_with("Spotify"));
        assert_eq!(
            rows[0].actions[0].command, "sub show 2",
            "a button targets the subscription it names"
        );
    }

    #[test]
    fn subtitle_carries_the_monthly_total_and_due_count() {
        let tile = tile(&sample(), today());
        assert_eq!(tile.subtitle.as_deref(), Some("$43.98/mo · 2 due"));
    }

    #[test]
    fn calm_state_drops_the_due_count() {
        let mut store = Store::default();
        store.add(new_sub("Figma".into(), Some(144.0), Cycle::YEARLY, date("2027-01-10")));
        let tile = tile(&store, today());
        assert_eq!(tile.subtitle.as_deref(), Some("$12.00/mo"));
    }

    #[test]
    fn subtitle_flags_subscriptions_with_no_price() {
        let mut store = Store::default();
        store.add(new_sub("AMC".into(), None, Cycle::MONTHLY, date("2027-05-15")));
        let tile = tile(&store, today());
        assert_eq!(tile.subtitle.as_deref(), Some("$0.00/mo · 1 no price"));
        assert_eq!(rows_of(&tile)[0].text, "AMC");
    }

    #[test]
    fn extra_subscriptions_collapse_into_a_rollup_line() {
        let mut store = Store::default();
        for (i, name) in ["A", "B", "C", "D", "E", "F"].iter().enumerate() {
            store.add(new_sub(
                (*name).to_string(),
                Some(10.0),
                Cycle::MONTHLY,
                date("2026-09-20").add_days(i as i64),
            ));
        }
        let tile = tile(&store, today());
        check_limits(&tile);
        assert_eq!(rows_of(&tile).len(), MAX_ROWS, "rows are capped");
        let text: Vec<&str> = tile
            .body
            .iter()
            .filter_map(|n| match n {
                TileNode::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(text, vec!["+2 more"]);
    }

    #[test]
    fn trial_rows_count_down_to_the_cancel_by_date() {
        let mut store = Store::default();
        store.add(new_sub("Audible".into(), Some(14.95), Cycle::MONTHLY, date("2026-10-01")));
        store.get_mut(1).unwrap().trial_ends = Some("2026-09-19".into());
        let tile = tile(&store, today());
        check_limits(&tile);
        let rows = rows_of(&tile);
        assert_eq!(rows[0].badge.unwrap().label, "2d", "a trial counts down the same way");
        assert_eq!(rows[0].actions[0].command, "sub show 1");
    }

    #[test]
    fn an_expired_trial_stays_visible_and_urgent() {
        let mut store = Store::default();
        store.add(new_sub("Audible".into(), Some(14.95), Cycle::MONTHLY, date("2026-10-01")));
        store.get_mut(1).unwrap().trial_ends = Some("2026-09-10".into());
        let tile = tile(&store, today());
        let rows = rows_of(&tile);
        assert_eq!(rows[0].badge.unwrap().label, "7d ago");
        assert_eq!(rows[0].badge.unwrap().tone, Some(TileTone::Danger));
    }

    #[test]
    fn paused_subscriptions_stay_off_the_tile() {
        let mut store = sample();
        store.get_mut(1).unwrap().paused = true;
        let tile = tile(&store, today());
        let names: Vec<&str> = rows_of(&tile).iter().map(|r| r.text).collect();
        assert!(!names.iter().any(|n| n.starts_with("Netflix")));
        assert_eq!(names.len(), 2);
    }

    #[test]
    fn empty_store_shows_an_alert_and_the_add_field() {
        let tile = tile(&Store::default(), today());
        check_limits(&tile);
        assert!(matches!(
            tile.body[0],
            TileNode::Component { ref component, .. } if component == "mui-alert"
        ));
        assert_eq!(tile.footer.len(), 1);
        assert_eq!(tile.footer[0].command, "sub add");
        assert_eq!(tile.footer[0].input.as_deref(), Some("Netflix 19.99 monthly"));
    }

    #[test]
    fn footer_offers_add_and_all() {
        let tile = tile(&sample(), today());
        let commands: Vec<&str> = tile.footer.iter().map(|a| a.command.as_str()).collect();
        assert_eq!(commands, vec!["sub add", "subs"]);
        assert_eq!(tile.footer[0].input.as_deref(), Some("Netflix 19.99 monthly"));
        assert!(tile.footer[1].input.is_none(), "plain buttons take no input");
    }

    #[test]
    fn an_unreadable_stored_date_gets_a_fix_action() {
        let mut store = sample();
        store.get_mut(2).unwrap().next_due = "soon".into();
        let tile = tile(&store, today());
        check_limits(&tile);
        let row = rows_of(&tile)
            .into_iter()
            .find(|r| r.text.starts_with("Spotify"))
            .expect("the broken row is still shown");
        assert_eq!(row.badge.unwrap().label, "date?");
        assert_eq!(row.actions[0].command, "sub show 2");
    }

    #[test]
    fn a_missing_clock_degrades_instead_of_lying() {
        let tile = tile(&sample(), None);
        check_limits(&tile);
        assert!(matches!(tile.body[0], TileNode::Text { .. }));
        assert_eq!(tile.footer[0].command, "subs");
    }

    #[test]
    fn long_names_and_many_subs_stay_inside_every_limit() {
        let mut store = Store::default();
        for i in 0..12 {
            store.add(new_sub(
                format!("Subscription with an unreasonably long product name number {i}"),
                Some(12345.67),
                Cycle::Days(45),
                date("2026-09-01").add_days(i),
            ));
        }
        store.currency = "CHF ".into();
        let tile = tile(&store, today());
        check_limits(&tile);
        let json = serde_json::to_string(&tile).unwrap();
        assert!(json.len() <= 16384, "tile JSON must stay under 16KB: {}", json.len());
        for row in rows_of(&tile) {
            assert!(row.text.chars().count() <= MAX_LABEL, "row label: {}", row.text);
            assert!(row.text.ends_with('…'), "long labels are elided: {}", row.text);
        }
    }

    #[test]
    fn no_countdown_nodes_are_emitted() {
        // check_limits panics on Countdown; assert it across several shapes.
        for store in [sample(), Store::default()] {
            check_limits(&tile(&store, today()));
        }
    }
}
