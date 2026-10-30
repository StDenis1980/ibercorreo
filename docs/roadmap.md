# Tinta Correo — milestones

Status: 2026-10-30 · Current focus: M4 (Microsoft Graph)

| Milestone | Scope | State |
|---|---|---|
| M0 | Skeleton: model, MIME, iCalendar, store, commands, undo, UI (mail, month calendar, people), CLI, CI | done |
| M1 | IMAP (RFC 9051/3501: folders, sync, IDLE, flags, move) and SMTP (RFC 5321, STARTTLS/TLS via rustls), account setup, OS keychain, offline queue (Outbox really sends), HTML mail shown safely | done (M1.1–M1.6: IMAP, TLS, two-way sync, SMTP and the Outbox, accounts in the window, safe HTML) except IDLE, which moves to M3 with push and autodiscovery |
| M2 | Calendar: week and day views, recurrence (RRULE), time zones, reminders, CalDAV sync (RFC 4791), meeting invitations (iTIP/iMIP: accept, decline); contacts: vCard, CardDAV | done: recurrence (M2.1), day and week views (M2.2), time zones (M2.3), reminders (M2.4), two-way CalDAV sync (M2.5), meeting invitations (M2.6), contact cards and vCard (M2.7a), two-way CardDAV sync (M2.7b). Organizing meetings (sending invitations) stays in gap 13 |
| M3 | OAuth2 sign-in (Microsoft 365, Gmail), autodiscovery (RFC 6186, Mozilla ISPDB format), full-text search index, rules and filters, signatures, multiple accounts | in progress: autoconfiguration and IDLE (M3.1), OAuth2 for Gmail and Microsoft 365 (M3.2), several accounts (M3.3), the full-text search index (M3.4), rules (M3.5) done; Outlook.com checked; Gmail by app password, Google verification after the public release |
| M4 | Exchange. Microsoft Graph first, since Exchange Online switches EWS off (from 2026-10, fully 2027-04-01): the Graph client and token (M4.1), the Outlook calendar (M4.2), contacts (M4.3), mail for work accounts without IMAP (M4.4); then EWS for on-premises Exchange (M4.5, Thunderbird's MPL-2.0 `ews` crate, unmodified) | in progress: M4.1 (Graph client, Graph consent in the Microsoft sign-in) M4.2 (Outlook calendars, both ways) and M4.3 (Outlook contacts, both ways) done |
| M5 | Migration from Outlook: import `.pst` and `.msg` (Microsoft Open Specifications MS-PST, MS-OXMSG); export; installers (Windows, macOS, Linux); user guide complete | Windows MSI done early (#20); `.pst`/`.msg`, macOS and Linux installers and the full guide open |
| M6 | MCP server (agent control), PGP/S-MIME (including signing mail with an FNMT certificate), accessibility pass, translations | translations done early (uk, es, ca, gl, eu, it, fr, de, pl: #24–#27; native review pending); the rest open |

## Current focus
M3: autoconfiguration and IDLE (M3.1), OAuth2 sign-in for Gmail and Microsoft 365 (M3.2) and several accounts with Favorites, signatures and Bcc (M3.3) landed, and so did the full-text search index with Outlook-style queries and scopes (M3.4) and mail rules (M3.5). Next, in order:

M4, decided 2026-10-30 (Graph first): M4.1 (client and token) and M4.2 (Outlook calendars both
ways) and M4.3 (Outlook contacts both ways) landed; a Microsoft account signed in before M4.1
signs in again once to give the calendar and contacts consent. Next M4.4, mail through Graph for
work accounts whose IMAP is switched off.

Earlier: Google verification, decided 2026-10-28: Gmail signs in with an app password for now (offered first in Account settings, with a link to make one); browser sign-in stays as the second choice: the Google OAuth app is published unverified (2026-10-28, owner's step in the Cloud Console), so no 7-day re-sign-in, an "unverified app" page the app explains, and at most 100 Google users. Google's verification and the paid CASA assessment wait until after the public release and the first support sales. Outlook.com browser sign-in works on a real account (checked 2026-10-28).

Then M4 (Exchange) and M5 (Outlook migration) in stage 2 of the suite plan (December 2026 – January 2027), M6 in 2027. The suite-wide work (releases, Linux, signing, tests, user guides) is in the Tinta Office roadmaps of the sibling repos (`docs/tinta-roadmap.md`).

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
- 2026-10-23 — M2.7b (CardDAV sync) landed; M2 done, focus M3.
- 2026-10-23 — M3.1 (autoconfiguration, IDLE) landed.
- 2026-10-23 — M3.2 (OAuth2 sign-in) landed.
- 2026-10-23 — M2.5b (CalDAV push) landed; M2.5 done.
- 2026-10-26 — M3.3 (several accounts) landed.
- 2026-10-26 — M3.3b (Favorites in the folder pane) landed.
- 2026-10-26 — M3.3c (signatures, Bcc, sync progress) landed.
- 2026-10-27 — Tinta Office roadmap review: M5 Windows MSI and M6 translations marked done early; Current focus lists M3.4, M3.5, the Outlook.com check and the Google verification decision.
- 2026-10-27 — M3.4 (full-text search index) landed; focus M3.5.
- 2026-10-27 — M3.5 (rules) landed.
- 2026-10-27 — `ibercorreo-cli sync` reads the credential store.
- 2026-10-28 — Gmail by app password first; Google verification deferred until after the public release and first sales; Outlook.com sign-in checked.
- 2026-10-28 — Gmail browser sign-in published unverified (choice between app password and Google sign-in).
- 2026-10-30 — M4 order: Microsoft Graph first, EWS last; M4.1 landed.
- 2026-10-30 — M4.2 landed.
- 2026-10-30 — M4.3 landed.
