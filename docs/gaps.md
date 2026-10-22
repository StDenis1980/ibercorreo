# Iber Correo — ranked gaps

Status: 2026-10-22 · Pick from the top.

1. **No network.** The IMAP protocol client exists (`crates/imap`, tested against a scripted server)
   but nothing connects yet: TLS (M1.2), sync into the store (M1.3) and SMTP (M1.4) are next.
   Outbox queues mail but nothing is sent.
2. **HTML mail** is reduced to plain text; links, inline images (`cid:`) and tables are lost. Needs a
   safe HTML-to-rich-text renderer (no remote fetch by default).
3. **Credentials**: no keychain integration yet (must never be stored in the store file).
4. **Recurring events** are stored (raw `RRULE`) but not expanded; only the first occurrence shows.
5. **Time zones**: `TZID` is kept, times are shown as written; `Date:` offsets of incoming mail are
   not applied.
6. **Undo** snapshots the whole store (50 steps). Fine for local mail; replace with operation-level
   undo before sync makes stores large.
7. **Store** is one JSON file loaded in memory. Move to an on-disk index (per-folder files or an
   embedded database) before IMAP sync of large mailboxes.
8. **Attachments**: names and sizes are kept, contents are not; no saving or opening yet (Iber PDF,
   Iber Texto, Iber Calculo hand-off planned).
9. **Localization**: the interface is English only; Ukrainian, Spanish and Portuguese planned.
10. **Week/day calendar views**, reminders, invitations (M2).

## Revision history
- 2026-10-22 — created with M0.
- 2026-10-22 — gap 1 narrowed: IMAP protocol core.
