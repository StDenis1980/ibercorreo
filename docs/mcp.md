# Tinta Correo MCP server

Status: 2026-11-04 · first version (M6.1; Vilna OS request, tintaoffice-web#28)

`ibercorreo-cli mcp` lets an AI assistant work with Tinta Correo's mail, calendar and contacts
over the [Model Context Protocol](https://modelcontextprotocol.io): newline-delimited JSON-RPC 2.0
on stdin and stdout (protocol `2025-06-18`; `2025-03-26` and `2024-11-05` clients are answered in
their own version). Logs go to stderr. The accounts, passwords and tokens stay in Correo: the
assistant never sees them and never needs a mail server of its own.

```sh
ibercorreo-cli mcp                       # the mailbox the app opens
ibercorreo-cli mcp --store FILE          # another mailbox
ibercorreo-cli mcp --sample              # the sample mailbox, in memory (never saved)
ibercorreo-cli mcp --allow-send          # also lets it send drafts the person approved
```

```json
{ "mcpServers": { "tinta-correo": { "command": "ibercorreo-cli", "args": ["mcp"] } } }
```

## One window per mailbox

When the Tinta Correo window is open on the mailbox, every command runs **in the window**: the
server hands it over through the window's instance port (`<store>.instance`, the same channel a
second start of the app uses, `crates/engine/src/instance.rs`), the window runs it like a click
and answers. The person sees the draft appear, the message move, the event added, and the
window's autosave writes the change; the server never loads or writes the mailbox while the
window has it.

When the window is closed, the server loads the mailbox itself, runs the command and saves what
it changed at once. It keeps the mailbox in memory between calls and reads it again when the file
changes. Before it saves, it checks again: if a window opened (or another program saved) while
the command ran, its own change is dropped and the command runs again in the window, or on what
was saved. A window that starts while the server is idle finds no lock and opens as usual.

A window too old to take commands, or one that does not answer within 30 seconds (it runs them
between frames), is a tool error; the command may still run there.

## Sending

Sending is a tool of its own, `send_draft`, and it is **off** unless the server starts with
`--allow-send`. No other tool sends, and there is no tool that runs an arbitrary command.

`send_draft` takes the draft's id and its `contentHash`: the fingerprint `read_message` returns,
over the account it is sent from, the sender, the recipients (To, Cc, Bcc), the subject, the text,
the formatted body, the meeting part and the attachments' names, types and sizes (`sha256:` and 64
hex digits). The assistant shows the person the draft, and passes the hash of what it showed; if
the draft changed after that (the person edited it in the window, another tool updated it), the
send is refused and the draft stays in Drafts. The check runs in `message.send` itself, in the
same step as the send, so nothing can change in between.

A sent draft goes to the Outbox. With the window open it goes out at once (the window starts a
Send/Receive); without it, at the window's next Send/Receive or `ibercorreo-cli sync`.

## Confining an assistant

- `--allow-commands LIST`: only the commands listed may run, by id (`message.read`) or group
  (`message.*`), separated by commas, spaces or new lines; `#` starts a comment. `@FILE` reads the
  list from a regular file of at most 256 KiB. An unknown id, a group that matches nothing or an
  empty list stops the server at startup, so a typo never widens or silently narrows the list.
  A tool is listed when a command it runs is allowed, and each call checks the command it runs
  (`reply_to_message` runs `message.reply`, or `message.replyAll` with `all`). `send_draft` needs
  `--allow-send` as well as `message.send` in the list.
- `--max-messages N` (1–499, default 50): most messages in one `list_messages` or
  `search_messages` answer; `more` says there are more, and `offset` pages through a folder.
  Contacts come at most 200 at a time, a message's text at most 100 000 characters (`maxChars`
  asks for less).
- Files: Correo keeps attachments' names, types and sizes, not their contents, so no tool reads
  or writes a file and there is no `--automation-write-root` yet; a tool that saves attachments
  will take one.

Each flag may be given once.

## Tools

| Tool | Command | Read-only |
|---|---|---|
| `list_folders` | `folder.list` | yes |
| `list_messages` (`folder`, `offset?`, `limit?`) | `message.list` | yes |
| `search_messages` (`query`, `scope?`, `limit?`) | `search.messages` | yes |
| `read_message` (`id`, `maxChars?`) | `message.read` (without the formatted copy of the body) | yes |
| `create_draft` (`to?`, `cc?`, `bcc?`, `subject?`, `body?`, `account?`) | `message.compose` | no |
| `reply_to_message` (`id`, `all?`, `body?`) | `message.reply` / `message.replyAll` | no |
| `forward_message` (`id`, `to?`, `body?`) | `message.forward` | no |
| `update_draft` (`id`, `to?`, `cc?`, `bcc?`, `subject?`, `body?`) | `message.update` | no |
| `send_draft` (`id`, `contentHash`) — only with `--allow-send` | `message.send` | no (destructive) |
| `set_flag` (`id`, `flagged?`), `mark_read` (`id`, `read?`) | `message.flag`, `message.markRead` | no |
| `archive_message` (`id`), `move_message` (`id`, `folder`) | `message.archive`, `message.move` | no |
| `delete_message` (`id`) | `message.delete` (Trash; in Trash, for good) | no (destructive) |
| `list_calendars`, `list_events` (`date?` or `from`+`to`) | `calendar.list`, `event.list` | yes |
| `add_event` (`summary`, `start`, …), `delete_event` (`id`, `occurrence?`) | `event.add`, `event.delete` | no |
| `list_contacts` (`query?`, `book?`), `get_contact` (`id`) | `contact.list`, `contact.get` | yes |
| `add_contact` (`name`, …), `update_contact` (`id`, …) | `contact.add`, `contact.update` | no |

Each tool's `annotations` carry `readOnlyHint` and `destructiveHint` as above (`openWorldHint` on
`send_draft`). `folder` is an id from `list_folders` or a role (`inbox`, `drafts`, `sent`,
`outbox`, `archive`, `trash`, `junk`). Results are one text block of pretty JSON; a refused or
failed command is `isError: true` with a readable message. An unknown tool, a missing required
argument, an argument the tool does not take or one of the wrong type is a JSON-RPC `-32602`
error; malformed JSON is `-32700`; an unknown method `-32601`.

Message text is other people's writing: the server returns it as data, and the instructions tell
the assistant to treat it as such. A host that marks external content (Vilna OS's Herkos does)
should mark every `read_message` and `search_messages` result.

## Example

```text
→ {"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}
→ {"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"list_messages","arguments":{"folder":"inbox","limit":5}}}
→ {"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"reply_to_message","arguments":{"id":3,"body":"Gracias, lo vemos el lunes."}}}
← … {"id": 12} …
→ {"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"read_message","arguments":{"id":12}}}
← … "contentHash": "sha256:5f0c…" …   (shown to the person, who approves)
→ {"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"send_draft","arguments":{"id":12,"contentHash":"sha256:5f0c…"}}}
```

## Not yet

- Contacts in Evolution Data Server for other GNOME apps (the calendar is there already,
  `docs/gnome.md`).
- Saving attachments (Correo does not keep their contents yet).
- Sending without the window right away: a closed window's Outbox goes out at its next
  Send/Receive.

## Revision history

- 2026-11-04 — first version: tools for mail, calendar and contacts; commands run in the open
  window; `send_draft` behind `--allow-send` and the draft's content hash; `--allow-commands`,
  `--max-messages`.
