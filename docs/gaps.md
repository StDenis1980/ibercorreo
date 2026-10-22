# Iber Correo — ranked gaps

Status: 2026-10-22 · Pick from the top.

1. **Mail accounts: the basics only.** Account settings, Send/Receive (background, at start,
   every 5 minutes, F9) and the password in the system credential store work, checked against
   Dovecot and Postfix. Missing: IDLE (push), autodiscovery and OAuth (M3: Gmail and Microsoft 365
   need it), more than one account, a progress bar. Sent copies are
   always saved to the server's Sent (servers that save them themselves, like Gmail, will need an
   account setting); no Bcc field; a message is refused as a whole if any recipient is.
   Known sync limits: a message moved between server folders is downloaded again (no COPYUID
   remapping); folders deleted on the server are kept here; one account only; whole messages are
   downloaded (no header-first fetch).
2. **HTML mail, the first pass.** HTML is read into safe formatted text (headings, lists, quotes,
   bold and italic, links with their real address on hover and a ⚠ on phishing-shaped ones);
   scripts, forms, styles and hidden preview text are dropped and remote images are never loaded.
   Missing: inline (`cid:`) images and a "load remote images from this sender" choice, tables laid
   out as tables (cells show as paragraphs), colours and fonts, a confirmation before opening a
   deceptive link, and a true bold weight (the interface font has one weight, so bold is shown in
   the strong text colour).
3. **Credentials**: passwords live in the system credential store; where none runs (a bare Linux
   session) they last only until the app closes. No OAuth yet (M3).
4. **Recurring events** expand (`DAILY`…`YEARLY` with `BYDAY`, `BYMONTHDAY`, `BYMONTH`, `BYSETPOS`,
   `COUNT`, `UNTIL`; `EXDATE`; changed occurrences by `RECURRENCE-ID`). Missing: editing one
   occurrence in the window (only deleting it), sub-daily and `BYWEEKNO`/`BYYEARDAY` rules (shown
   once), `RDATE`, and exceptions matched by day only (one occurrence a day at most).
5. **Time zones** work: events in a zone (IANA, Windows names as Outlook writes them, vendor
   ids) and in UTC show in the zone chosen (default: the computer's), repeating events follow
   summer time in their own zone, and mail dates are kept in UTC and shown locally. Missing:
   reading `VTIMEZONE` (a zone only Outlook's own definition describes, like "Customized Time
   Zone", shows as written), writing `VTIMEZONE` for repeating events (one-off ones are exported in
   UTC; the IANA id is enough for Google, Apple and Thunderbird), a second zone column in the day
   view, and choosing an appointment's zone in the form (it takes the calendar's).
6. **Undo** snapshots the whole store (50 steps), and a sync clears the history (stepping back past
   a sync would resurrect stale server state). Replace with operation-level undo.
7. **Store** is one JSON file loaded in memory. Move to an on-disk index (per-folder files or an
   embedded database) before IMAP sync of large mailboxes.
8. **Attachments**: names and sizes are kept, contents are not; no saving or opening yet (Iber PDF,
   Iber Texto, Iber Calculo hand-off planned).
9. **Localization**: the interface is English only; Ukrainian, Spanish and Portuguese planned.
10. **Calendar views**: day, work week, week and month work. Missing: dragging an appointment to
    move or stretch it, a working-hours setting (the grid opens at 07:00), a week starting on
    Sunday, and reminders and invitations (M2).

## Revision history
- 2026-10-22 — created with M0.
- 2026-10-22 — gap 1 narrowed: IMAP protocol core.
- 2026-10-22 — gap 1 narrowed: TLS connections.
- 2026-10-22 — gap 1 narrowed: two-way sync (CLI); gap 6 notes undo is cleared by sync.
- 2026-10-22 — gap 1 narrowed: SMTP sending (CLI).
- 2026-10-22 — gap 1 narrowed: accounts in the window (M1.5); gap 3 closed (keychain).
- 2026-10-22 — gap 2 narrowed: safe HTML display (M1.6).
- 2026-10-22 — gap 4 narrowed: recurrence (M2.1).
- 2026-10-22 — gap 10 narrowed: day and week views (M2.2).
- 2026-10-22 — gap 5 narrowed: time zones (M2.3).
