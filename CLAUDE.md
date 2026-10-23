# Tinta Correo — notes for Claude

Read [AGENTS.md](AGENTS.md) first: its rules (assets, clean room, never crash, privacy) bind you.

- Start from `ROADMAP.md`, `docs/gaps.md` and `docs/roadmap.md` (Current focus).
- Every user-visible action is a command with tests (`crates/engine/src/commands.rs`).
- Look at UI changes: `cargo run -p ibercorreo -- --sample`.
- Headless: `cargo run -p ibercorreo-cli -- run --sample --cmd message.list --cmd 'event.list={"date":"today"}'`.
- Gate before each commit: `cargo xtask ci`. Commit per task id (`M1.2: IMAP IDLE`).
- `main` is never pushed to directly: branch, PR, merge when CI is green.
