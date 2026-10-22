# Iber Correo — roadmap summary

Status: 2026-10-22 · M1 (IMAP/SMTP) in progress

**Stage:** M1. Offline mail, calendar and people in one window; two-way IMAP sync and SMTP sending work from the CLI; the window gets account setup in M1.5.

| Number | Value |
|---|---|
| Commands in the registry | 35 |
| Tests | see `cargo test --workspace` |
| Protocols working | IMAP4rev1/rev2 two-way sync and SMTP submission over TLS/STARTTLS (CLI); files: `.eml`, `.ics` |

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

## Revision history
- 2026-10-22 — created.
- 2026-10-22 — M1.1.
- 2026-10-22 — M1.2.
- 2026-10-22 — M1.3.
- 2026-10-22 — M1.4.
