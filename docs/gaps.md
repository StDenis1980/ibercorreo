# Tinta Correo — ranked gaps

Status: 2026-10-26 · Pick from the top.

1. **Mail accounts: the basics only.** Account settings, Send/Receive (background, at start,
   every 5 minutes, F9) and the password in the system credential store work, checked against
   Dovecot and Postfix. Find settings (autoconfig files, the public database, the usual server
   names) and IDLE push on the Inbox work (M3.1), and so does OAuth sign-in for Gmail and
   Microsoft 365 (M3.2), once a build carries registered client IDs (`docs/oauth.md`: Google's
   needs verification and a security assessment before anyone but test users can sign in).
   Several accounts sit side by side, each with its own folders, Send/Receive and IDLE watch
   (M3.3); choosing Google or Microsoft (or typing their address) fills in the servers; a folder
   with more than 500 new messages keeps downloading until it is complete. Calendars and contacts
   sync with one account's server at a time. Missing: a progress bar, a unified Inbox, moving
   mail between accounts, DNS SRV (RFC 6186) and MX lookups (no DNS
   resolver yet), a separate CardDAV/CalDAV address in Find settings, OAuth for Google's
   calendars and contacts, other OAuth providers. Sent copies are saved to the server's Sent
   except on Gmail, which files them itself (Microsoft 365 may too: unchecked, an account setting
   will cover others); no Bcc field; a message is refused as a whole if any recipient is.
   Known sync limits: a message moved between server folders is downloaded again (no COPYUID
   remapping); folders deleted on the server are kept here; whole messages are
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
8. **Attachments**: names and sizes are kept, contents are not; no saving or opening yet (Tinta PDF,
   Tinta Texto, Tinta Calculo hand-off planned).
9. **Localization**: the interface is English only; Ukrainian, Spanish and Portuguese planned.
10. **Calendar views**: day, work week, week and month work. Missing: dragging an appointment to
    move or stretch it, a working-hours setting (the grid opens at 07:00), a week starting on
    Sunday.
11. **Reminders** fire in the window (Dismiss, Snooze, Dismiss All; dismissals saved, missed ones
    kept two days) and travel in `.ics` as `VALARM`. Missing: a system notification and a sound
    when the window is in the background or closed (needs a tray/background process), a
    per-person default other than 15 minutes, and reminders on mail (follow-up flags).
12. **CalDAV sync works both ways (M2.5).** Calendars are found from a server, principal or
    calendar address; at Send/Receive changes made here go up (each only if the server's copy is
    unchanged; otherwise the server's copy wins and comes down) and changes there come down
    (only what changed). Events written back keep what Tinta Correo does not read. Checked
    against Radicale. Missing: telling the person which change lost a conflict (only a count in
    the status bar), `sync-collection` (RFC 6578) instead of listing every ETag, a separate
    CalDAV password, tasks (`VTODO`), creating, renaming or hiding a calendar, moving an
    appointment to another calendar, and checking against Nextcloud, Google and iCloud.
13. **Meeting invitations, as an attendee (M2.6).** An invitation by mail (iMIP) shows a strip with
    Accept, Tentative and Decline; the meeting goes into (or out of) the calendar, an update moves
    the same event, and the answer (`METHOD:REPLY`) goes to the organizer, sent now, edited first
    or not at all. A cancellation can be removed, a whole series or one day. Checked end to end:
    an invitation sent through Postfix, accepted into a Radicale calendar, the answer delivered.
    Missing: sending invitations (organizing a meeting: attendees in the appointment form,
    `REQUEST`/`CANCEL` out, applying `REPLY`s to the attendee list), showing tentative meetings
    differently, `COUNTER` (propose a new time) and `DELEGATED`, conflicts with the calendar shown
    in the strip, the invitation's attendees and organizer kept on the event (a server calendar gets
    the meeting without them), server-side scheduling (CalDAV `schedule-inbox`, RFC 6638), and
    moving an already accepted meeting to the calendar chosen when answering again.
14. **Contacts (M2.7).** Cards with several addresses and phones, organization, job
    title, birthday and notes, edited in People; `.vcf` import (vCard 2.1 from Outlook, 3.0, 4.0)
    and export (3.0), keeping every property not shown (postal addresses, photos, web sites) so a
    round trip loses nothing. Address books synced both ways with a CardDAV server (M2.7b, the
    same address as the calendars), checked against Radicale. Missing: checking against
    Nextcloud, Google (needs OAuth and its own CardDAV address, M3) and iCloud; a separate
    CardDAV address when it differs from the CalDAV one; creating, renaming and deleting address
    books (on the server too); `sync-collection` (RFC 6578) instead of a full listing on every
    change; cards written back as 3.0 even when the server held 4.0; contact groups and lists, photos
    shown, postal addresses and web sites edited, address completion in the To field, an
    "add sender to contacts" action, birthdays in the calendar, and an export button in the
    window (export is a command). A multi-part organization (`ORG:Company;Department`) is kept
    as one text ("Company, Department").

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
- 2026-10-22 — gap 11 added: reminders (M2.4).
- 2026-10-23 — gap 12 added: CalDAV pull (M2.5a).
- 2026-10-23 — gap 12 narrowed: CalDAV push (M2.5b).
- 2026-10-23 — gap 13 added: meeting invitations (M2.6); gap 10 no longer lists them.
- 2026-10-23 — gap 14 added: contacts and vCard (M2.7a).
- 2026-10-23 — gap 14 narrowed: CardDAV sync (M2.7b).
- 2026-10-23 — gap 1 narrowed: autoconfiguration and IDLE (M3.1).
- 2026-10-23 — gap 1 narrowed: OAuth sign-in for Gmail and Microsoft 365 (M3.2).
- 2026-10-26 — gap 1 narrowed: several accounts, servers filled in for Google and Microsoft, downloads past 500 (M3.3).
