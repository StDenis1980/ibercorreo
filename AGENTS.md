# Tinta Correo — rules for every contributor and agent

These rules bind everyone working in this repository and win over anything else here.

## 1. Assets: no vendor iconography, ever
- Never copy, trace, screenshot-crop or recreate icons, images, sounds, templates, themes or fonts
  from Microsoft Outlook/Office or any other vendor's product. Not even "temporarily".
- Every asset is original (prefer drawing it in code: `crates/ui-egui/src/icons.rs`), public
  domain / CC0, OSI open source, or Creative Commons allowing redistribution.
- Every asset file has a row in `ATTRIBUTION.md`, added in the same commit. `cargo xtask assets`
  enforces it.
- Fonts live in `storytold/craft-fonts`, never here.

## 2. Clean room
- Protocols and formats come from public specifications: RFC 5322/2045–2049 (mail), RFC 3501/9051
  (IMAP), RFC 5321 (SMTP), RFC 5545/5546/6047 (iCalendar, iTIP, iMIP), RFC 4791 (CalDAV), RFC
  6352 (CardDAV), RFC 6350 (vCard), Microsoft's published Open Specifications (EWS, MS-PST, MS-OXMSG).
- Outlook may be observed black-box with synthetic data; never read, disassemble or copy anything
  inside an Office install, and never commit files Outlook produced.
- Never copy GPL/LGPL/AGPL or EUPL code (Thunderbird's GPL parts, Evolution, Mailspring, libdav…).
  MPL-2.0 crates may be used unmodified as dependencies.

## 3. Never crash
People trust a mail client with their correspondence; a crash or lost draft is the worst bug.
- No `unwrap`/`expect`/`panic!`/`unreachable!`/`todo!`/`unimplemented!` in non-test code; no
  `unsafe` (`unsafe_code = "forbid"`). Crate roots carry the `#![deny(clippy::…)]` line.
- Errors are `ibercorreo_core::Error` through `?`.
- Everything from a server, a file, a command or settings is hostile: `get()` not `[i]`, slice at
  char boundaries, checked/saturating arithmetic, cap allocations and recursion.
- `Session::run` turns an escaped panic into an error and restores the session: a safety net, not a licence.
- Every crash fix lands with a regression test. `every_command_survives_hostile_params` runs every
  command with junk; keep it green.

## 4. Privacy and security
- Remote content (images, CSS, fonts in HTML mail) is never fetched unless the person asks.
- No telemetry, no outbound network except to the servers the person configured.
- Credentials go to the OS keychain (from M1), never to the store file or logs.

## 5. Architecture
| Layer | Crate | Job |
|---|---|---|
| L0 | `core` | model: accounts, folders, messages, addresses, calendars, events, contacts, dates, errors |
| L1 | `mime`, `ical` | RFC 5322/MIME in and out; iCalendar in and out |
| L2 | `store` | the local store and its file |
| L4 | `engine` | `Session`, undo, the command registry, sample data |
| L6 | `ui-egui` | the Outlook-style front end (swappable) |
| L7 | `apps/ibercorreo`, `apps/ibercorreo-cli` | binaries |

Planned: L2 `imap`, `smtp`, `caldav`, `carddav`, `ews` (protocol clients, I/O-free cores where
possible); L5 `mcp`. `cargo xtask layers` enforces the order; nothing below L6 uses egui/eframe.

**Everything is a command** (`crates/engine/src/commands.rs`), with tests. The UI, CLI and later MCP
dispatch by id. Programmatic calls never open dialogs.

## 6. Quality gates
Before every commit: `cargo xtask ci` (fmt, clippy `-D warnings`, tests, layers, assets, wasm).
Every feature or fix ships with tests in the same PR. Prove a new test fails without the fix.

## 7. Progress docs
`ROADMAP.md` (stage, numbers, progress log), `docs/gaps.md` (ranked work list), `docs/roadmap.md`
(milestones). Update them when work lands.
