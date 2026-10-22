# Iber Correo — roadmap summary

Status: 2026-10-22 · M2 (calendar and contacts) in progress

**Stage:** M1 done. Mail, calendar and people in one window; an IMAP/SMTP account set up in the window sends and receives in the background, and HTML mail is shown formatted and safe.

| Number | Value |
|---|---|
| Commands in the registry | 50 |
| Tests | see `cargo test --workspace` |
| Protocols working | IMAP4rev1/rev2 two-way sync and SMTP submission over TLS/STARTTLS; files: `.eml`, `.ics` |

Detail: [docs/roadmap.md](docs/roadmap.md) (milestones), [docs/gaps.md](docs/gaps.md) (ranked work list).

## Progress log
- 2026-10-22 — M0: workspace, model, MIME in/out, iCalendar in/out, store, engine with 35 commands
  and undo, Outlook-style egui UI, CLI, icon, CI gate.
- 2026-10-22 — M1.1: `ibercorreo-imap` — response framing with capped literals, typed responses,
  injection-safe commands, client (LOGIN only over TLS, LIST with special-use roles, SELECT, UID
  SEARCH/FETCH/STORE/MOVE/EXPUNGE, APPEND, STARTTLS with the plaintext-injection check), modified
  UTF-7 names, scripted-server and hostile-server tests.
- 2026-10-22 — M1.2: `ibercorreo-net` — TCP with timeouts, rustls (ring) with the OS certificate
  verifier, implicit TLS and STARTTLS IMAP sessions; local-TLS tests and a real Dovecot check
  (`docs/testing.md`). IMAP `CREATE`.
- 2026-10-22 — M1.3: `ibercorreo-sync` — two-way sync (new mail newest first, flags both ways,
  moves, deletes with tombstones, UIDVALIDITY resets), store format 2 (`remote` on folders and
  messages), `Session::apply_sync`, `ibercorreo-cli sync`; checked end to end against Dovecot.
- 2026-10-22 — M1.4: `ibercorreo-smtp` (RFC 5321/6409: EHLO, STARTTLS, AUTH PLAIN/LOGIN, SIZE,
  8BITMIME, dot-stuffing, injection-safe addresses), `net::smtp`, Outbox sending with Sent copies
  (`sync::outbox`, `Session::outbox`/`apply_sent`); checked against Postfix 3.8 delivering to Dovecot.
- 2026-10-22 — M1.5: `account.set`/`account.get`/`account.sync` (F9), Account settings dialog,
  Send/Receive on a background thread (`net::job`) at start, every 5 minutes and on request,
  passwords in the system credential store (`keyring`); `ibercorreo-cli sync` uses the store's account.
- 2026-10-22 — M1.6: safe HTML mail (`mime::html` into `core::rich`: no scripts, forms, styles,
  hidden text or remote loads; `http`/`https`/`mailto` links only, phishing-shaped links marked),
  formatted reading pane with "Show as plain text", `message.read` and `view.plainText`. M1 done.
- 2026-10-22 — M2.1: repeating events (`core::recur`: RFC 5545 `RRULE` expansion, bounded), `EXDATE`
  and `RECURRENCE-ID` in `.ics`, occurrences in the month view and agenda, `event.add` `repeat`/`rrule`/
  `until`, `event.delete` `occurrence`, a Repeat choice in the appointment form.
- 2026-10-22 — M2.2: Day, Work Week and Week views (a time grid with overlapping appointments side
  by side, an all-day strip and a now line), `calendar.view*` (Ctrl+Alt+1…4), `calendar.next`/`prev`
  stepping by the view, `event.list` over a range (`from`/`to`, or `from: "view"`). 45 commands.
- 2026-10-22 — M2.3: time zones (`core::zone` over `jiff` with the IANA database bundled; Windows
  zone names and vendor ids understood): events shown in the chosen zone (`calendar.timeZone`,
  default the computer's), repeating events expand in their own zone across summer time, new
  appointments carry the calendar's zone, one-off zoned events export in UTC, mail `Date:` kept in
  UTC and shown locally, a zone picker on the calendar ribbon. 46 commands.
- 2026-10-22 — M2.4: reminders (`VALARM` read in every trigger form and written; `Event.reminders`;
  `reminder.list`/`dismiss`/`snooze`, `event.setReminder`, `event.add` `reminder` defaulting to 15
  minutes; dismissals saved in the store; missed reminders kept two days; an Outlook-style
  reminder window and a Reminder choice in the appointment form). 50 commands.

## Revision history
- 2026-10-22 — created.
- 2026-10-22 — M1.1.
- 2026-10-22 — M1.2.
- 2026-10-22 — M1.3.
- 2026-10-22 — M1.4.
- 2026-10-22 — M1.5.
- 2026-10-22 — M1.6; M1 done.
- 2026-10-22 — M2.1.
- 2026-10-22 — M2.2.
- 2026-10-22 — M2.3.
- 2026-10-22 — M2.4.
