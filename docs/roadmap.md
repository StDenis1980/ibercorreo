# Iber Correo — milestones

Status: 2026-10-22 · Current focus: M1

| Milestone | Scope | State |
|---|---|---|
| M0 | Skeleton: model, MIME, iCalendar, store, commands, undo, UI (mail, month calendar, people), CLI, CI | done |
| M1 | IMAP (RFC 9051/3501: folders, sync, IDLE, flags, move) and SMTP (RFC 5321, STARTTLS/TLS via rustls), account setup, OS keychain, offline queue (Outbox really sends), HTML mail shown safely | in progress: IMAP protocol (M1.1), TLS connections (M1.2) two-way sync (M1.3) and SMTP sending (M1.4) done, CLI only until M1.5 |
| M2 | Calendar: week and day views, recurrence (RRULE), time zones, reminders, CalDAV sync (RFC 4791), meeting invitations (iTIP/iMIP: accept, decline); contacts: vCard, CardDAV | |
| M3 | OAuth2 sign-in (Microsoft 365, Gmail), autodiscovery (RFC 6186, Mozilla ISPDB format), full-text search index, rules and filters, signatures, multiple accounts | |
| M4 | Exchange: EWS mail (Thunderbird's MPL-2.0 `ews` crate, unmodified), then EWS calendar and contacts, then Microsoft Graph | |
| M5 | Migration from Outlook: import `.pst` and `.msg` (Microsoft Open Specifications MS-PST, MS-OXMSG); export; installers (Windows, macOS, Linux); user guide complete | |
| M6 | MCP server (agent control), PGP/S-MIME, accessibility pass, translations (uk, es, pt, en) | |

## Current focus
M1: IMAP/SMTP accounts, so Iber Correo reads and sends real mail.

## Revision history
- 2026-10-22 — created with M0.
- 2026-10-22 — M0 done; M1.1 (IMAP protocol core) landed.
- 2026-10-22 — M1.2 (TLS connections, checked against Dovecot) landed.
- 2026-10-22 — M1.3 (two-way IMAP sync) landed.
- 2026-10-22 — M1.4 (SMTP sending and the Outbox) landed.
