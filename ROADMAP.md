# Iber Correo — roadmap summary

Status: 2026-10-22 · M1 (IMAP/SMTP) in progress

**Stage:** M1. Offline mail, calendar and people in one window; the IMAP client is built but not wired to a connection yet.

| Number | Value |
|---|---|
| Commands in the registry | 35 |
| Tests | see `cargo test --workspace` |
| Protocols working | IMAP4rev1/rev2 client core (no TLS connection yet); files: `.eml`, `.ics` |

Detail: [docs/roadmap.md](docs/roadmap.md) (milestones), [docs/gaps.md](docs/gaps.md) (ranked work list).

## Progress log
- 2026-10-22 — M0: workspace, model, MIME in/out, iCalendar in/out, store, engine with 35 commands
  and undo, Outlook-style egui UI, CLI, icon, CI gate.
- 2026-10-22 — M1.1: `ibercorreo-imap` — response framing with capped literals, typed responses,
  injection-safe commands, client (LOGIN only over TLS, LIST with special-use roles, SELECT, UID
  SEARCH/FETCH/STORE/MOVE/EXPUNGE, APPEND, STARTTLS with the plaintext-injection check), modified
  UTF-7 names, scripted-server and hostile-server tests.

## Revision history
- 2026-10-22 — created.
- 2026-10-22 — M1.1.
