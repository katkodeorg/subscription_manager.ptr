# Subscription Manager

A Pointiv extension that tracks what you pay for and when it renews. A tile beside the
popup command bar shows what is overdue and what is due next. A passed deadline counts
up, `3d ago`, and keeps counting until you mark it renewed.

Rust/WASM, built on [`pointiv-extension-sdk`](https://crates.io/crates/pointiv-extension-sdk) 0.4.

## Install

Paste this repository's GitHub URL into Pointiv → Settings → Extensions:

```text
https://github.com/<your-username>/subscription_manager.ptr
```

The tile appears in the right-hand zone the next time you open the popup. Move it,
resize it, or turn it off in Settings → Tiles.

## The tile

The card is 300px wide and each row is one line, so rows stay short: the name, a
countdown, and **Inspect**. Four rows, most urgent first.

```text
Subscriptions
$19.99/mo · 2 no price

  Peacock          [  74d  ]  (Inspect)
  AMC A-List       [ 239d  ]  (Inspect)
  Netflix          [ 3d ago]  (Inspect)
  +1 more
  [Add ______]  [All]
```

The countdown turns amber inside seven days. It turns red once the date passes and
counts up from there: `3d ago`, `12d ago`.

Replies to commands are one line. The detail sits behind **Inspect**, which opens it in
the result panel:

```text
## AMC A-List
- Price not recorded, monthly
- **Free trial, cancel by 2027-05-15**, in 239 days
- It starts charging you that day
- Card: Bank of America CREDIT

Say what you want: "cancel amc", "amc is paid", "snooze amc a week".
```

The footer has an **Add** field. Type `Netflix 19.99 monthly` into it. **All** opens the
full list. Row five onward collapses into `+1 more`.

## How you reach it

Pointiv matches an extension against what you type. It never matches against your
selection. The match is a keyword prefix or a substring of the description. `sub`,
`track`, `trial`, `cancel`, `due`, `bill`, `spend`, `expiry`, and `receipt` all
surface it.

A full command line with arguments matches nothing. Typing `sub cancel 1` into the bar
finds no extension. There are three ways in:

1. **The tile.** Inspect on any row, and the Add field in the footer.
2. **Select text, type `sub`, pick Subscription Manager.** It reads the selection and
   tracks it.
3. **Plain English.** Type `cancel amc` or `netflix is paid` and press Enter with no
   suggestion selected. Pointiv's AI orchestrator passes it to the extension, which
   translates it into a command.

## Commands

These are the commands the extension understands. Reach them through the three routes
above.

| Command | What it does |
|---------|--------------|
| `subs` | Overdue, due soon, upcoming, paused, and the monthly total |
| `sub add <name> [price] [cycle]` | `sub add Netflix 19.99 monthly`, `sub add Figma 144/yr` |
| `sub trial <name> <date>` | `sub trial AMC may 15 2027 card Bank of America credit` |
| `sub track` | Reads a subscription out of selected text, such as a receipt |
| `sub renew <name>` | Marks it paid, rolls the date forward, records the payment |
| `sub snooze <name> [days]` | Pushes the deadline, records nothing (default 3 days) |
| `sub pause` / `sub resume <name>` | Drops it from totals and the tile, or brings it back |
| `sub edit <name> <field> <value>` | `price`, `cycle`, `next`, `cat`, `card`, `url`, `trial`, `note`, `name`, `remind`, `cal` |
| `sub remove <name>` / `sub undo` | Delete, or take back the last change (undo also redoes) |
| `sub show <name>` | Everything known, plus the commands that apply. The tile's Inspect button |
| `sub spend` | Monthly and yearly totals, category split, budget status |
| `sub trials` | Free trials by cancel-by date |
| `sub cancel <name>` | Returns the cancellation link on its own, so ⌘C copies it |
| `sub card <text>` | Everything billed to one card, for when it expires |
| `sub cal <name\|all>` | Creates a recurring Google Calendar reminder |
| `sub export` / `sub import` | JSON backup (⌘C copies it) and restore from selected text |
| `sub budget <amount>` | Sets a monthly cap that `sub spend` reports against |
| `sub remind <days>` | How far ahead new calendar reminders fire (default 3, max 28) |
| `sub tz <hours>` | Your UTC offset, e.g. `sub tz -7`. See *Dates* below |
| `sub currency <symbol>` | `$`, `£`, `CHF` |
| `sub help` | The list above |

Names match case-insensitively by exact name, unique prefix, or unique substring. Ids
always work. An ambiguous name comes back with the list of matches.

### Tracking from selected text

Select a line anywhere, open Pointiv, and type `sub` or `track`:

```text
AMC A-List free trial on Bank of America CREDIT card, first charge May 15 2027
```

The AI reads the name, the card, and the date. It records a trial to cancel by
15 May 2027.

### Trials and unknown prices

A price is optional. Typing it by hand looks like this:

```text
sub trial AMC may 15 2027 card Bank of America CREDIT
```

A date with no price is read as a trial. A date with a price is read as a paid renewal.
The reply says which one it chose and gives the command to flip it:

```text
sub add AMC may 15 2027        -> trial, cancel by 2027-05-15
sub add Netflix 19.99 may 15 2027  -> paid, renews 2027-05-15

sub trial Netflix              flips a paid entry to a trial, same date
sub edit AMC trial none        flips a trial to a paid subscription
```

Fill in a price later with `sub edit AMC price 12.99`. Totals skip entries with no
price and report how many there are.

Cards are free text, so `Bank of America CREDIT`, `BoA credit`, and `4242` all work.
`sub card boa` matches any of them.

### Add options

```text
sub add Netflix 19.99 monthly next 2026-10-03 cat streaming card 4242 \
  url https://netflix.com/cancelplan trial +14d remind 5 note joint account
```

Cycles: `monthly`, `yearly`, `weekly`, `quarterly`, `biweekly`, `6 months`, `45 days`,
`/mo`, `/yr`.

Dates: `YYYY-MM-DD`, `may 15 2027`, `15 May 2027`, `today`, `tomorrow`, `+10d`, `+2w`.

A date can sit anywhere in the line, with or without a `next` keyword. It always means
the end date: the day the next charge lands, or the day a trial has to be cancelled by.
A month and day with no year means the next time that date comes around. With no date at
all, the next charge is set one full cycle out and the reply names the date it picked.

### Plain English

Type a sentence and Pointiv's AI orchestrator hands it to the extension, which
translates it into one of the commands above and echoes what it ran:

```text
i just paid for netflix   →  _Read as `sub renew netflix`._
```

## Calendar reminders

Pointiv extensions run when you open the popup. Notifications come from Google Calendar.
`sub cal <name>` creates an all-day event with a reminder the configured number of days
ahead.

A free trial gets one event on its cancel-by date, titled `Cancel AMC before it charges
you`, with the card in the description. Everything else gets a recurring event on the
renewal date. It fires for every future renewal.

Requires Google connected under Pointiv → Settings → Account. The event id is stored, so
a second `sub cal` skips it.

## Dates

The extension reads a UTC clock. Run `sub tz -7` once with your UTC offset in hours.
That makes "today" match your day. Monthly cycles clamp to the end of short months.
31 January plus one month is 28 February.

## Storage

Two keys in this extension's sandboxed store at `~/.pointiv/extensions/<id>/storage/`.
`data` holds the subscription list. `undo` holds the document before the last change.
Data stays on your machine. Calendar events go to Google. `sub track` and plain English
send text to Pointiv's AI.

## Limits

- **The extension runs when you open the popup or press a tile button.** The tile
  refreshes at those moments. Calendar events cover the rest of the time.
- **An extension can create calendar events. It cannot update or delete them.** After
  changing a cycle or date, run `sub edit <name> cal none`, delete the old event in
  Google Calendar, then `sub cal <name>` again.
- **Copy flows use ⌘C.** Pointiv discards `Output::copy` for community extensions.
  `sub cancel` and `sub export` return their payload on its own line and ⌘C copies the
  result.
- **Typed arguments need the tile or plain English.** The command bar matches extensions
  by keyword prefix. Typing `sub add Netflix 19.99 monthly` in the bar matches nothing.
  Use the tile's **Add** field, or describe it in a sentence.

## Build

Needs [Rust](https://rustup.rs). `./build.sh` writes `extension.wasm` to the repo root.
Commit that file so Pointiv can load it from GitHub.

```sh
cargo test     # date maths, parsing, store operations, tile limits
./build.sh
```

For a local loop, point Pointiv → Settings → Tiles → dev extension folder at this
directory and press load. The manifest pins a SHA-256 of the wasm and verifies it on
every run, so reload after every build.

## Permissions

| Permission | Used for |
|------------|----------|
| `storage` | The subscription list and the undo snapshot |
| `ai` | `sub track` and plain-English requests |
| `google_calendar` | `sub cal` renewal reminders |

The manifest does not request `network`.
