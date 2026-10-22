# Iber Correo — ranked gaps

Status: 2026-10-22 · Pick from the top.

1. **Sync and sending only from the CLI.** Two-way IMAP sync (`crates/sync`, checked against
   Dovecot) and SMTP sending (`ibercorreo-cli sync --smtp-host`, checked against Postfix) work, but
   the window has no account setup or sync button yet (M1.5). No IDLE (push) yet. Sent copies are
   always saved to the server's Sent (servers that save them themselves, like Gmail, will need an
   account setting); no Bcc field; a message is refused as a whole if any recipient is.
   Known sync limits: a message moved between server folders is downloaded again (no COPYUID
   remapping); folders deleted on the server are kept here; one account only; whole messages are
   downloaded (no header-first fetch).
2. **HTML mail** is reduced to plain text; links, inline images (`cid:`) and tables are lost. Needs a
   safe HTML-to-rich-text renderer (no remote fetch by default).
3. **Credentials**: no keychain integration yet (must never be stored in the store file).
4. **Recurring events** are stored (raw `RRULE`) but not expanded; only the first occurrence shows.
5. **Time zones**: `TZID` is kept, times are shown as written; `Date:` offsets of incoming mail are
   not applied.
6. **Undo** snapshots the whole store (50 steps), and a sync clears the history (stepping back past
   a sync would resurrect stale server state). Replace with operation-level undo.
7. **Store** is one JSON file loaded in memory. Move to an on-disk index (per-folder files or an
   embedded database) before IMAP sync of large mailboxes.
8. **Attachments**: names and sizes are kept, contents are not; no saving or opening yet (Iber PDF,
   Iber Texto, Iber Calculo hand-off planned).
9. **Localization**: the interface is English only; Ukrainian, Spanish and Portuguese planned.
10. **Week/day calendar views**, reminders, invitations (M2).

## Revision history
- 2026-10-22 — created with M0.
- 2026-10-22 — gap 1 narrowed: IMAP protocol core.
- 2026-10-22 — gap 1 narrowed: TLS connections.
- 2026-10-22 — gap 1 narrowed: two-way sync (CLI); gap 6 notes undo is cleared by sync.
- 2026-10-22 — gap 1 narrowed: SMTP sending (CLI).
