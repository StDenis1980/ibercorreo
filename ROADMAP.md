# Tinta Correo — roadmap summary

Status: 2026-10-23 · M3 (accounts) in progress

**Stage:** M1 and M2 done. Mail, calendar and people in one window, with calendars and contacts kept in step with a CalDAV/CardDAV server; an IMAP/SMTP account set up in the window sends and receives in the background, and HTML mail is shown formatted and safe.

| Number | Value |
|---|---|
| Commands in the registry | 59 |
| Tests | see `cargo test --workspace` |
| Protocols working | IMAP4rev1/rev2 two-way sync with IDLE push and SMTP submission over TLS/STARTTLS, with a password or OAuth 2.0 (XOAUTH2: Gmail, Microsoft 365); CalDAV two-way calendar sync; CardDAV two-way contact sync; meeting invitations by mail (iTIP/iMIP: accept, tentative, decline, cancel); files: `.eml`, `.ics`, `.vcf` |

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
- 2026-10-23 — M2.5a: CalDAV pull. New `ibercorreo-dav` crate (HTTP/1.1 with capped replies,
  WebDAV `multistatus` with bounded XML, CalDAV discovery via `/.well-known/caldav`, principal and
  calendar home; `calendar-query` ETags; `calendar-multiget`), `net::dav` over TLS,
  `sync::calendar` (only changed calendars are listed, only changed events fetched; calendars
  gone from the server removed), `account.set` `caldav`, calendar colours and a calendar list in
  the window, Send/Receive on the calendar ribbon. Checked against Radicale 3.8.
- 2026-10-23 — M2.5b: CalDAV push. Appointments added to a server calendar, changed or deleted
  here are written back (`PUT` with `If-None-Match`/`If-Match`, `DELETE` with `If-Match`), only
  inside their calendar; the server's copy wins a conflict; offline changes wait for the next
  sync. Written-back events keep what Tinta Correo does not read (attendees, organizer,
  categories, `VTIMEZONE`, the exact `RECURRENCE-ID`; `ical::write_resource`), with a fresh
  `DTSTAMP` and `SEQUENCE`. `EXDATE` now takes `DTSTART`'s form. A Calendar choice in the
  appointment form, `event.add` `calendar`, `calendar.list`, unique UIDs. 51 commands.
- 2026-10-23 — M2.6: meeting invitations. A message's `text/calendar` part is kept
  (`Message.invite`); `ical::invite` reads `METHOD`, `ORGANIZER`, `ATTENDEE` and `SEQUENCE`, and
  `ical::reply` writes the `METHOD:REPLY` for one attendee. `invite.respond` (accept, tentative,
  decline; answer sent now, edited first or not at all) puts the meeting in a calendar or takes it
  out and queues the answer, sent as `multipart/alternative` with the calendar part;
  `invite.remove` takes a cancelled meeting (or one day of it) out. An invitation strip in the
  reading pane; a sample invitation. Checked end to end through Postfix, Dovecot and Radicale.
  53 commands.
- 2026-10-23 — M2.7a: contacts. New `ibercorreo-vcard` crate (vCard 2.1 with quoted-printable in
  Windows-1252, 3.0 and 4.0 read; 3.0 written; unread properties kept and written back; bounded).
  Contacts hold several phones, a UID, job title, note and birthday (store format 3; format 2
  phones carried over). `contact.update`, `contact.delete`, `contact.get`, `contact.importVcf`,
  `contact.exportVcf`; `.vcf` files open with the app; an editable contact card in People.
  58 commands.
- 2026-10-23 — M2.7b: CardDAV. `dav::carddav` (discovery through `/.well-known/carddav`, the
  principal and `addressbook-home-set`; Depth 1 ETag listing; `addressbook-multiget`; `PUT` as
  `text/vcard`), `sync::contacts` (both ways, like calendars: only changed books listed, only
  changed cards fetched; contacts added, changed or deleted here written back with
  `If-Match`/`If-None-Match`; the server's card wins a conflict; unread vCard lines go back
  unchanged). Address books in the store (`books`, card tombstones), `contact.listBooks`, a `book`
  on `contact.add`/`update`/`list`/`importVcf` (moving a card between books deletes it on the
  old server), the CalDAV address serves CardDAV too. People shows the address books and
  Send/Receive; the contact card has an Address book choice. `Url::join` now resolves `.` and `..`
  segments, so a server's href cannot step outside its collection. Checked against Radicale 3.8,
  from the CLI and the window. 59 commands. M2 done.
- 2026-10-23 — M3.1: autoconfiguration and push mail. `dav::autoconfig` reads Mozilla's
  `clientConfig` format (secure IMAP and SMTP only, address templates filled in, OAuth-only
  providers marked); `net::autoconfig` looks at the provider, the domain's `.well-known`, then
  the public Thunderbird database (domain only), following HTTPS-only redirects, and falls back
  to probing `imap.`/`mail.`/`smtp.` names. Find settings in Account settings,
  `ibercorreo-cli autoconfig`. IMAP `IDLE` (`idle_start`/`idle_wait`/`idle_done`, read timeouts
  survive mid-line), `net::idle::watch` on its own read-only connection, and a watcher thread in
  the app that starts a Send/Receive within seconds of new mail, reconnecting with backoff.
  Checked against Dovecot in the window and on six real database files.
- 2026-10-23 — M3.2: OAuth 2.0 sign-in for Gmail and Microsoft 365. `AUTHENTICATE XOAUTH2` in
  the IMAP client (SASL-IR or not, the error challenge answered) and `AUTH XOAUTH2` in the SMTP
  client; `net::oauth`: the browser flow from RFC 8252 (a one-time listener on 127.0.0.1, PKCE
  S256, a random state), the code and refresh exchanges, and a shared `Credential` that renews
  access tokens before they run out and keeps rotated refresh tokens. The account remembers its
  provider (`account.set` `oauth`); Account settings picks it for Gmail and Microsoft servers and
  signs in with Save and sign in; the refresh token lives in the system credential store; a
  revoked grant asks to sign in again. Client IDs come from the build or the environment
  (`docs/oauth.md`); `ibercorreo-cli oauth`. Checked end to end with a test provider: Dovecot
  signing in by XOAUTH2 (sync and IDLE) and Postfix sending by it.

## Revision history
- 2026-10-22 — created.
- 2026-10-22 — M1.1.
- 2026-10-22 — M1.2.
- 2026-10-22 — M1.3.
- 2026-10-22 — M1.4.
- 2026-10-22 — M1.5.
- 2026-10-22 — M1.6; M1 done.
- 2026-10-23 — M2.5a.
- 2026-10-23 — M2.5b.
- 2026-10-23 — M2.6.
- 2026-10-23 — M2.7a.
- 2026-10-23 — M2.7b; M2 done.
- 2026-10-23 — M3.1.
- 2026-10-23 — M3.2.
- 2026-10-22 — M2.1.
- 2026-10-22 — M2.2.
- 2026-10-22 — M2.3.
- 2026-10-22 — M2.4.
