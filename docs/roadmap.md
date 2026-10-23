# Iber Correo — milestones

Status: 2026-10-23 · Current focus: M2

| Milestone | Scope | State |
|---|---|---|
| M0 | Skeleton: model, MIME, iCalendar, store, commands, undo, UI (mail, month calendar, people), CLI, CI | done |
| M1 | IMAP (RFC 9051/3501: folders, sync, IDLE, flags, move) and SMTP (RFC 5321, STARTTLS/TLS via rustls), account setup, OS keychain, offline queue (Outbox really sends), HTML mail shown safely | done (M1.1–M1.6: IMAP, TLS, two-way sync, SMTP and the Outbox, accounts in the window, safe HTML) except IDLE, which moves to M3 with push and autodiscovery |
| M2 | Calendar: week and day views, recurrence (RRULE), time zones, reminders, CalDAV sync (RFC 4791), meeting invitations (iTIP/iMIP: accept, decline); contacts: vCard, CardDAV | in progress: recurrence (M2.1), day and week views (M2.2), time zones (M2.3), reminders (M2.4), two-way CalDAV sync (M2.5), meeting invitations (M2.6), contact cards and vCard (M2.7a) done; next: CardDAV sync (M2.7b) |
| M3 | OAuth2 sign-in (Microsoft 365, Gmail), autodiscovery (RFC 6186, Mozilla ISPDB format), full-text search index, rules and filters, signatures, multiple accounts | |
| M4 | Exchange: EWS mail (Thunderbird's MPL-2.0 `ews` crate, unmodified), then EWS calendar and contacts, then Microsoft Graph | |
| M5 | Migration from Outlook: import `.pst` and `.msg` (Microsoft Open Specifications MS-PST, MS-OXMSG); export; installers (Windows, macOS, Linux); user guide complete | |
| M6 | MCP server (agent control), PGP/S-MIME, accessibility pass, translations (uk, es, pt, en) | |

## Current focus
M2: contacts synced with CardDAV servers (M2.7b), the last part of M2.

## Revision history
- 2026-10-22 — created with M0.
- 2026-10-22 — M0 done; M1.1 (IMAP protocol core) landed.
- 2026-10-22 — M1.2 (TLS connections, checked against Dovecot) landed.
- 2026-10-22 — M1.3 (two-way IMAP sync) landed.
- 2026-10-22 — M1.4 (SMTP sending and the Outbox) landed.
- 2026-10-22 — M1.5 (account settings, background Send/Receive, credential store) landed.
- 2026-10-22 — M1.6 (safe HTML mail) landed; M1 done, IDLE moved to M3; focus M2.
- 2026-10-22 — M2.1 (recurrence) landed.
- 2026-10-22 — M2.2 (day, work week and week views) landed.
- 2026-10-22 — M2.3 (time zones) landed.
- 2026-10-22 — M2.4 (reminders) landed.
- 2026-10-23 — M2.5a (CalDAV pull) landed; Current focus brought up to date.
- 2026-10-23 — M2.5b (CalDAV push) and M2.6 (meeting invitations) landed.
- 2026-10-23 — M2.7a (contact cards, vCard) landed.
- 2026-10-23 — M2.5b (CalDAV push) landed; M2.5 done.
