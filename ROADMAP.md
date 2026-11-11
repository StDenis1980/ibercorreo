# Tinta Correo — roadmap summary

Status: 2026-11-04 · M4 (Exchange) done up to a real-server check; M5 (Outlook migration) started: M5.1 `.msg` import, M5.1b compressed-RTF bodies, M5.2 `.pst` mail (also from the window), M5.2b `.pst` calendar and contacts; M6.1 MCP server (`ibercorreo-cli mcp`)

**Stage:** M1 and M2 done. Mail, calendar and people in one window, with calendars and contacts kept in step with a CalDAV/CardDAV server; an IMAP/SMTP account set up in the window sends and receives in the background, and HTML mail is shown formatted and safe.

| Number | Value |
|---|---|
| Commands in the registry | 76 |
| Tests | see `cargo test --workspace` |
| Protocols working | IMAP4rev1/rev2 two-way sync with IDLE push and SMTP submission over TLS/STARTTLS, with a password or OAuth 2.0 (XOAUTH2: Gmail, Microsoft 365); CalDAV two-way calendar sync; CardDAV two-way contact sync; meeting invitations by mail (iTIP/iMIP: accept, tentative, decline, cancel); files: `.eml`, `.ics`, `.vcf` |

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
- 2026-10-22 — M1.5: `account.set`/`account.get`/`account.sync` (F9), Account settings dialog,
  Send/Receive on a background thread (`net::job`) at start, every 5 minutes and on request,
  passwords in the system credential store (`keyring`); `ibercorreo-cli sync` uses the store's account.
- 2026-10-22 — M1.6: safe HTML mail (`mime::html` into `core::rich`: no scripts, forms, styles,
  hidden text or remote loads; `http`/`https`/`mailto` links only, phishing-shaped links marked),
  formatted reading pane with "Show as plain text", `message.read` and `view.plainText`. M1 done.
- 2026-10-22 — M2.1: repeating events (`core::recur`: RFC 5545 `RRULE` expansion, bounded), `EXDATE`
  and `RECURRENCE-ID` in `.ics`, occurrences in the month view and agenda, `event.add` `repeat`/`rrule`/
  `until`, `event.delete` `occurrence`, a Repeat choice in the appointment form.
- 2026-10-22 — M2.2: Day, Work Week and Week views (a time grid with overlapping appointments side
  by side, an all-day strip and a now line), `calendar.view*` (Ctrl+Alt+1…4), `calendar.next`/`prev`
  stepping by the view, `event.list` over a range (`from`/`to`, or `from: "view"`). 45 commands.
- 2026-10-22 — M2.3: time zones (`core::zone` over `jiff` with the IANA database bundled; Windows
  zone names and vendor ids understood): events shown in the chosen zone (`calendar.timeZone`,
  default the computer's), repeating events expand in their own zone across summer time, new
  appointments carry the calendar's zone, one-off zoned events export in UTC, mail `Date:` kept in
  UTC and shown locally, a zone picker on the calendar ribbon. 46 commands.
- 2026-10-22 — M2.4: reminders (`VALARM` read in every trigger form and written; `Event.reminders`;
  `reminder.list`/`dismiss`/`snooze`, `event.setReminder`, `event.add` `reminder` defaulting to 15
  minutes; dismissals saved in the store; missed reminders kept two days; an Outlook-style
  reminder window and a Reminder choice in the appointment form). 50 commands.
- 2026-10-23 — M2.5a: CalDAV pull. New `ibercorreo-dav` crate (HTTP/1.1 with capped replies,
  WebDAV `multistatus` with bounded XML, CalDAV discovery via `/.well-known/caldav`, principal and
  calendar home; `calendar-query` ETags; `calendar-multiget`), `net::dav` over TLS,
  `sync::calendar` (only changed calendars are listed, only changed events fetched; calendars
  gone from the server removed), `account.set` `caldav`, calendar colours and a calendar list in
  the window, Send/Receive on the calendar ribbon. Checked against Radicale 3.8.
- 2026-10-23 — M2.5b: CalDAV push. Appointments added to a server calendar, changed or deleted
  here are written back (`PUT` with `If-None-Match`/`If-Match`, `DELETE` with `If-Match`), only
  inside their calendar; the server's copy wins a conflict; offline changes wait for the next
  sync. Written-back events keep what Tinta Correo does not read (attendees, organizer,
  categories, `VTIMEZONE`, the exact `RECURRENCE-ID`; `ical::write_resource`), with a fresh
  `DTSTAMP` and `SEQUENCE`. `EXDATE` now takes `DTSTART`'s form. A Calendar choice in the
  appointment form, `event.add` `calendar`, `calendar.list`, unique UIDs. 51 commands.
- 2026-10-23 — M2.6: meeting invitations. A message's `text/calendar` part is kept
  (`Message.invite`); `ical::invite` reads `METHOD`, `ORGANIZER`, `ATTENDEE` and `SEQUENCE`, and
  `ical::reply` writes the `METHOD:REPLY` for one attendee. `invite.respond` (accept, tentative,
  decline; answer sent now, edited first or not at all) puts the meeting in a calendar or takes it
  out and queues the answer, sent as `multipart/alternative` with the calendar part;
  `invite.remove` takes a cancelled meeting (or one day of it) out. An invitation strip in the
  reading pane; a sample invitation. Checked end to end through Postfix, Dovecot and Radicale.
  53 commands.
- 2026-10-23 — M2.7a: contacts. New `ibercorreo-vcard` crate (vCard 2.1 with quoted-printable in
  Windows-1252, 3.0 and 4.0 read; 3.0 written; unread properties kept and written back; bounded).
  Contacts hold several phones, a UID, job title, note and birthday (store format 3; format 2
  phones carried over). `contact.update`, `contact.delete`, `contact.get`, `contact.importVcf`,
  `contact.exportVcf`; `.vcf` files open with the app; an editable contact card in People.
  58 commands.
- 2026-10-23 — M2.7b: CardDAV. `dav::carddav` (discovery through `/.well-known/carddav`, the
  principal and `addressbook-home-set`; Depth 1 ETag listing; `addressbook-multiget`; `PUT` as
  `text/vcard`), `sync::contacts` (both ways, like calendars: only changed books listed, only
  changed cards fetched; contacts added, changed or deleted here written back with
  `If-Match`/`If-None-Match`; the server's card wins a conflict; unread vCard lines go back
  unchanged). Address books in the store (`books`, card tombstones), `contact.listBooks`, a `book`
  on `contact.add`/`update`/`list`/`importVcf` (moving a card between books deletes it on the
  old server), the CalDAV address serves CardDAV too. People shows the address books and
  Send/Receive; the contact card has an Address book choice. `Url::join` now resolves `.` and `..`
  segments, so a server's href cannot step outside its collection. Checked against Radicale 3.8,
  from the CLI and the window. 59 commands. M2 done.
- 2026-10-23 — M3.1: autoconfiguration and push mail. `dav::autoconfig` reads Mozilla's
  `clientConfig` format (secure IMAP and SMTP only, address templates filled in, OAuth-only
  providers marked); `net::autoconfig` looks at the provider, the domain's `.well-known`, then
  the public Thunderbird database (domain only), following HTTPS-only redirects, and falls back
  to probing `imap.`/`mail.`/`smtp.` names. Find settings in Account settings,
  `ibercorreo-cli autoconfig`. IMAP `IDLE` (`idle_start`/`idle_wait`/`idle_done`, read timeouts
  survive mid-line), `net::idle::watch` on its own read-only connection, and a watcher thread in
  the app that starts a Send/Receive within seconds of new mail, reconnecting with backoff.
  Checked against Dovecot in the window and on six real database files.
- 2026-10-23 — M3.2: OAuth 2.0 sign-in for Gmail and Microsoft 365. `AUTHENTICATE XOAUTH2` in
  the IMAP client (SASL-IR or not, the error challenge answered) and `AUTH XOAUTH2` in the SMTP
  client; `net::oauth`: the browser flow from RFC 8252 (a one-time listener on 127.0.0.1, PKCE
  S256, a random state), the code and refresh exchanges, and a shared `Credential` that renews
  access tokens before they run out and keeps rotated refresh tokens. The account remembers its
  provider (`account.set` `oauth`); Account settings picks it for Gmail and Microsoft servers and
  signs in with Save and sign in; the refresh token lives in the system credential store; a
  revoked grant asks to sign in again. Client IDs come from the build or the environment
  (`docs/oauth.md`); `ibercorreo-cli oauth`. Checked end to end with a test provider: Dovecot
  signing in by XOAUTH2 (sync and IDLE) and Postfix sending by it.
- 2026-10-26 — M3.3: several accounts. `AccountId`; every folder belongs to an account, each
  with its seven standard folders (store format 4; older stores read as one account).
  `account.add`, `account.remove`, `account.list`, an `id` on `account.set`/`account.get`; a
  changed server or sign-in name forgets the mail synced from the old one; mail is written,
  answered, sent and filed in its own account, a move across accounts is refused, and search
  spans them all. Calendars and contacts sync with one account's server at a time. The folder
  pane groups folders under each address, with ⚙ settings, **+ Add account** and Remove account;
  choosing Google or Microsoft, or typing a Gmail/Outlook address, fills in the servers at once.
  The app syncs every account in turn, with its own password or OAuth grant and its own IDLE
  watch, opens the settings of the one whose sign-in failed, and keeps downloading while a
  folder has more than the 500 newest still on the server (the status says how many).
  `ibercorreo-cli sync --account ID`. Checked against Dovecot with two accounts (3 and 520
  messages). 63 commands.
- 2026-10-26 — M3.3b: the folder pane as Outlook lays it out. Favorites at the top (a right
  click on any folder adds it or takes it out; `folder.favorite`, `favorite` in `folder.list`;
  kept in the store, in the order added, gone with their folder or account; the sample starts
  with Inbox and Sent), each account a heading that folds, folder icons drawn in code, and
  unread counts on every folder. 64 commands.
- 2026-10-26 — M3.3c: a signature per account (Account settings, `signature` in `account.set`,
  added under new mail, replies and forwards above the quoted text; `compose` takes
  `signature:false`), a Bcc field (the **Bcc** button in compose; blind copies go into the SMTP
  envelope and never into the headers; checked against Postfix and Dovecot), and a progress bar in
  the status bar while syncing (account, folder, "N of M"). 64 commands.
- 2026-10-27 — M3.4: full-text search. `store::search`: an in-memory index of every message's
  words (subject, sender, recipients, body, attachment names), folded (case, Latin accents, the
  Catalan middle dot; Cyrillic letters kept apart) and read again only for messages that changed
  (process-unique stamps, so undo snapshots and copies stay right); queries with prefix words,
  `"phrases"`, `-word`, `OR`, `from:`/`to:`/`cc:`/`subject:`/`body:`/`attachment:` (with Ukrainian
  and Spanish names), `has:attachment`, `is:unread|read|flagged`, `after:`/`before:`/`on:`.
  `search.messages` takes a `scope` (all, mailbox, folder); the window has Outlook's scope list
  beside the search box and a syntax tooltip.
- 2026-10-27 — M3.5: rules. `store::rules`: a condition in search syntax and actions (mark read,
  flag, move, delete, stop processing more rules), in order, per account or for all; they run on
  mail new in an Inbox already synced (sync's `Report::arrived`: newer than the newest UID held,
  never the first download or older mail fetched later) and on any folder on request.
  `rule.list`/`add`/`set`/`delete`/`move`/`run` (70 commands); a Rules window (list, on/off,
  order, edit, Run rules now) and "Create rule…" on a message's right-click menu. "+ Add account"
  left the ribbon (it is in the folder pane), so the search box has room again.
- 2026-10-27 — `ibercorreo-cli sync` takes the password or OAuth refresh token from the
  environment, else from the system credential store entry the app keeps
  (`Account::credential_entry`, shared by both), and writes a rotated refresh token back there.
- 2026-10-28 — Folder pane: with several accounts, all but the one in use open folded, so a
  second account is not lost under the first one's folders (seen with 76 Outlook folders above
  Gmail). Adding an address that is already set up closes the dialog and opens that account.
- 2026-10-28 — Gmail: Account settings offer an app password first (`OAuthProvider::app_password_url`,
  link to Google's page under the password); browser sign-in stays as a choice. Google's OAuth
  verification and CASA are deferred until after the public release (owner decision).
- 2026-10-28 — Gmail users choose: app password, or Google sign-in with the OAuth app published
  unverified (no 7-day expiry, ≤100 users). `OAuthProvider::unverified` makes Account settings
  explain Google's "hasn't verified this app" page (Advanced → Go to Tinta Correo).
- 2026-10-28 — Big mailboxes (43,000 messages) no longer freeze the window: the message list lays
  out only the rows in view, a message is found by binary search, unread counts take one pass a
  frame, messages are shared (`Arc`) between the store, its undo steps and the copy a save writes,
  and the autosave and the search index run on background threads. A frame went from 3 s to
  ~25 ms, a command from 0.5 s to ~10 ms.
- 2026-10-28 — Messages are saved in shard files beside the store file (`store.mail/m000042.json`,
  a thousand ids each, format 5); a save rewrites only the shards whose messages changed (found by
  `Arc` identity against the last save), so the 1 GB single-file limit is gone. Older stores open
  as before and are written back sharded; a save cut short opens with the stray messages left out.
- 2026-10-28 — The search index keeps each distinct word once and a word's postings as packed
  numbers (message id and field mask) instead of every message's own copy of its words: with
  60,000 messages it takes 0.7 GB instead of 4.3 GB (the app held ~2 GB for 67,000 real ones).
- 2026-10-30 — M3.9: message text stays on disk. Each shard is two files: the headers
  (`m000042.json`) and the content (`c000042.json`: body, formatted body, invitation; format 6).
  A store opens with headers only (the list shows a stored preview) and reads a message's text
  when it is shown, replied to, sent or searched (`Store::content`, `content_mut` for drafts); a
  flag change rewrites the headers only. Removed messages keep their text until the store is
  opened again (undo), then it is dropped. A synthetic 67,000-message mailbox went from 1.65 GB
  in memory to 0.4 GB (35 MB of it the messages, the rest the search index); it opens in 0.1 s and
  a message reads in ~6 ms. Older stores are rewritten at once (`Store::needs_rewrite`).
- 2026-10-30 — M4.1: Microsoft Graph client (`dav::graph`): JSON requests with Graph's error
  codes, throttled requests waited out (`Retry-After`, at most a minute, then `Busy`), lists read
  page by page with their delta links, every page kept on the Graph host. The Microsoft sign-in
  now also asks consent for Graph (profile, calendar, contacts, mail); the Graph token is a second
  token renewed from the same refresh token, and a grant from before is `NeedsConsent` (sign in
  again) without stopping mail. `ibercorreo-cli graph` shows who is signed in or any Graph `GET`.
- 2026-10-30 — M4.2: Outlook calendars over Microsoft Graph for Microsoft accounts (no CalDAV
  address needed), both ways and shared with the CalDAV store side: each calendar with its
  colour, events in their own zone (Windows names), Outlook repeat patterns as `RRULE` and back,
  moved and deleted occurrences from `instances` (a year back, two ahead) as overrides and
  `EXDATE`s, changes sent as `POST`/`PATCH`/`DELETE` with `If-Match` (the server's copy wins a
  conflict), a deleted or moved occurrence here sent as that instance. Calendars from Graph and
  from CalDAV never remove each other (`Batch::scope`). `Zone::windows_name`.
- 2026-10-30 — M4.3: Outlook contacts over Microsoft Graph, both ways and shared with the CardDAV
  store side: the Contacts folder and each subfolder as an address book; names, up to three
  addresses, mobile, work and home numbers, company, job title, notes and birthday; changes sent
  as `POST`/`PATCH`/`DELETE` with `If-Match`; what Outlook cannot keep is reported. One Graph
  token serves the calendars and the contacts; CardDAV and Graph books never remove each other.
- 2026-11-01 — M4.4: mail over Microsoft Graph for Microsoft 365 work accounts whose IMAP is
  turned off (Account settings → Mail over Microsoft Graph, Microsoft sign-in only). Folders and
  subfolders by id, the standard ones by their well-known names (Exchange's Outbox left out);
  messages listed newest first with read and flag state and downloaded as MIME, so the IMAP store
  side and the rules are shared; Graph ids are immutable ones, kept beside a number given here
  (`RemoteRef::id`, tombstones too). Read, flagged, moves and deletions go back (`PATCH`, `move`,
  `permanentDelete`); the Outbox goes out with `sendMail` (MIME, Bcc as a header, 4 MB at most)
  and Exchange files the Sent copy. One Graph token for mail, calendar and contacts; no IDLE (the
  5-minute sync). Switching an account to Graph or back forgets the mail synced the other way.
- 2026-11-01 — M4.5a: mail over Exchange Web Services for an Exchange Server the organisation
  runs itself (Account settings → Exchange server (EWS): its name or EWS address, password
  sign-in, IMAP and SMTP not used). Requests and replies are Thunderbird's MPL-2.0 `ews` crate,
  unmodified; a reply is checked to be a SOAP envelope with a header before that crate reads it
  (it would stop the program otherwise), and a flag goes back as `PidTagFlagStatus` because the
  crate writes `item:Flag` outside the EWS namespace. Graph and EWS share one sync
  (`sync::server_mail::MailServer`): folders from `SyncFolderHierarchy` (mail folders only,
  Outlook's Outbox left out), `FindItem` pages newest first, `GetItem` MIME, `UpdateItem`,
  `MoveItem`, `DeleteItem` (`HardDelete`); the Outbox goes out with `CreateItem`
  (`SendAndSaveCopy`). Throttling (`ErrorServerBusy`, 503) waits for the next sync. Checked
  against synthetic Exchange 2016 replies only.
- 2026-11-01 — M4.5b, part 1: Exchange Autodiscover (plain-XML form, MS-OXDSCLI). Account
  settings → Find the Exchange server asks `https://<domain>/autodiscover/autodiscover.xml`, then
  `https://autodiscover.<domain>/…`, signed in with the name and password, follows address, URL
  and HTTP redirects (HTTPS only, at most 10) and takes the outside EWS address first. No DNS SRV
  or plain-HTTP redirect step.
- 2026-11-01 — M4.5b, part 2: Exchange calendars and contacts over EWS, both ways, for an
  account whose mail goes over EWS (no CalDAV address needed). Calendar folders of class
  `IPF.Appointment` and contacts folders of `IPF.Contact` (Exchange's hidden ones and the
  birthday calendar left out); items read as Exchange exports them (`MimeContent`: iCalendar,
  vCard, or a message with a `text/calendar` part) so the CalDAV/CardDAV store side is shared;
  written back as Exchange imports them (`CreateItem` with `MimeContent`, written by hand since
  the `ews` crate has no `SendMeetingInvitations`); a change replaces the item (create, then
  delete) only while its `ChangeKey` is the one last seen, else Exchange's copy wins. No
  invitations or cancellations sent. Turning EWS off forgets that server's calendars and books.
- 2026-11-01 — M4.5c: NTLM sign-in (NTLMv2, from the published MS-NLMP specification) for
  Exchange and any HTTPS server: when a server refuses Basic and offers NTLM, the client signs
  each connection in with NTLM (negotiate, challenge, answer on the same connection) and keeps
  it. The answer is bound to the server's TLS certificate (`tls-server-end-point`, RFC 5929), as
  Exchange's Extended Protection requires. `DOMAIN\user` or `user@domain` as the sign-in name.
  Checked against the specification's test values and a test HTTPS server that verifies the
  answer itself; no real Exchange server yet. No Kerberos, no message integrity code.
- 2026-11-03 — M5.1: Outlook `.msg` import. New crate `ibercorreo-outlook` reads compound files
  (MS-CFB: both sector sizes, DIFAT, mini stream; loops, overlong sizes and broken chains
  refused) and `.msg` messages (MS-OXMSG): SMTP sender and recipients (Exchange-only addresses
  filled from the saved Internet headers), subject and text in the message's code page, date,
  read state, plain and HTML body, attachment names, types and sizes (attached messages named by
  their subject). Contacts, appointments and tasks saved as `.msg` are refused. Command
  `message.importMsg`; opening a `.msg` file imports it (Windows "Open with", Linux MIME type);
  `ibercorreo-cli msg FILE`. A body kept only as compressed RTF is not read yet.
- 2026-11-03 — M5.1b: bodies Outlook kept only as compressed RTF. `ibercorreo-outlook::rtf`
  unpacks `PidTagRtfCompressed` (MS-OXRTFCP: LZFu with its checksum, and stored `MELA` data;
  the specification's examples pass) and takes the RTF apart (MS-OXRTFEX): HTML mail comes back
  as its original HTML (`\fromhtml1`, `\htmltag` groups, `\htmlrtf` parts dropped), plain-text
  and RTF-native mail as text in the RTF's code page, with `\uN` characters. An HTML property
  still wins, and the plain text Outlook saved stays the text. Formatting of RTF-native mail is
  not kept.
- 2026-11-03 — `.msg`: the HTML body is read in `PidTagInternetCodepage`, which can differ from
  the code page of the other text (found on a real Ukrainian message: text in Windows-1251, HTML
  in KOI8-U). Seven real messages from the owner (English and Ukrainian, HTML, RTF, attachments)
  now import correctly; the files are not kept.
- 2026-11-03 — M5.2 (engine and CLI): Outlook data files (`.pst`, MS-PST). `ibercorreo-outlook`
  reads the node database (Unicode and ANSI files, no/permute/cyclic encoding, node and block
  B-trees, data trees, subnodes; `.ost` refused), heaps, B-trees on heaps, property and table
  contexts, the folder tree under the IPM subtree, and messages through the same layer as
  `.msg` (recipients, attachments, attached messages). It reads from disk as it goes, so large
  archives do not have to fit in memory. Checked against the specification's sample pages and a
  test writer covering both formats, all encodings, multi-block heaps and two-level trees;
  damaged files never panic. The engine puts each mail folder in a local folder named
  `Archive/Inbox/…` and skips messages already imported; calendar, contact, task and note items
  are counted and left for M5.2b. Command `message.importPst`, `ibercorreo-cli pst FILE --store
  STORE`. The owner's own test archive (6 folders, 9 messages in Ukrainian with attachments)
  imports fully; the file is not kept. Not yet in the window.
- 2026-11-03 — HTML mail built from layout templates (MJML and similar) no longer shows empty:
  `font-size: 0` on a table cell hid all the text inside, even where it set a size again. It now
  hides only text, and a descendant's size undoes it; images still show. Found on a real bank
  notification in the owner's archive.
- 2026-11-03 — M5.2 in the window: a `.pst` opened with the app ("Open with" on Windows) or
  dropped on the window is read on another thread, with progress in the status bar, then put
  in the store; files dropped on the window import like files opened with it. New command
  `folder.delete` (also in a folder's context menu) removes one of the person's own local
  folders with its mail, undoable; standard and server folders stay.
- 2026-11-03 — M5.2b: calendar and contacts from `.pst`. `ibercorreo-outlook` reads the archive's
  named-property map (MS-PST node 0x61) and turns appointments into events (start and end in the
  zone Outlook stored, all-day days, reminders, the meeting UID, and series from the MS-OXOCAL
  recurrence pattern: daily, weekly, monthly, nth weekday, yearly, with deleted and moved
  occurrences as exceptions) and contacts into address-book entries (addresses, phones, postal
  addresses, company, title, birthday, web page, notes). Each calendar or contacts folder of the
  archive becomes a local calendar or address book of the same name; a second import adds
  nothing. Tasks, notes and contact groups are counted and left out.

## Revision history
- 2026-11-04 — M6.1: MCP server, `ibercorreo-cli mcp` (Vilna OS request, tintaoffice-web#28;
  `docs/mcp.md`). Tools for mail (list, search, read, drafts, replies, forwards, flags, move,
  archive, delete), calendar and contacts, with read-only and destructive hints. With the window
  open, commands run there through the instance port (`engine::instance`, one window per
  mailbox); without it the server loads the mailbox, saves each change and gives way to a window
  that opens meanwhile. Sending is its own tool, off unless `--allow-send`, and sends only a draft
  whose `contentHash` (new in `message.read`, checked by `message.send`) still matches what was
  shown. `--allow-commands`, `--max-messages`. Also: `message.list` takes `offset`/`limit`,
  `search.messages` `limit`, replies and forwards a `body`.
- 2026-11-04 — Correo's contacts in an Evolution Data Server address book "Tinta Correo" (GNOME
  Contacts and other GNOME apps see them; tintaoffice-web#28 item 5), kept equal by UID through
  the address book's D-Bus API, checked against EDS 3.52. The automatic Send/Receive period can be
  chosen (list beside Send/Receive: only by hand, 1–60 minutes; command `account.syncInterval`,
  kept in the mailbox; default 5).
- 2026-11-06 — Fix: standard folders linked to mailboxes the server does not have (an Outlook.com
  account whose Sent, Drafts and Trash pointed at `[Gmail]/…` paths). A sync with the server's whole
  list unlinks every folder whose mailbox is gone, keeps its mail here, moves mail synced from it
  into a folder of its own, and folds a standard folder into the one already mirroring the server's
  Sent/Trash… (RFC 6154, else by name), with its Favorites place and rules. Sent copies are saved
  only to a mailbox the server lists.
- 2026-11-05 — New look, after Outlook for macOS with our own icons: search in the title bar,
  a light rail (Mail, Calendar, People, settings), a pane of folders, calendars or address books
  with its New button, one row of labelled tools per view, white cards on a window washed with the
  theme's colour, and My Day (month and the coming days) on the right of Mail and Calendar. Six
  colour themes (teal, terracotta, sea, lavender, olive, azulejo), each light or dark or as the
  computer (`view.theme`), and buttons that hide the folder pane and My Day (`view.folderPane`,
  `view.myDay`); all kept in the mailbox. People is a table of contacts beside the contact card.
  Sizes after Fluent 2 (14 px text, 12 px captions, controls 28 px and up, a 4 px grid), dialogs
  and menus in the theme's colours, and the interface's size (`view.zoom`, 80–200 %, 110 % by
  default; Ctrl+= / Ctrl+- / Ctrl+0, or Size in the theme menu).
- 2026-10-22 — created.
- 2026-10-22 — M1.1.
- 2026-10-22 — M1.2.
- 2026-10-22 — M1.3.
- 2026-10-22 — M1.4.
- 2026-10-22 — M1.5.
- 2026-10-22 — M1.6; M1 done.
- 2026-10-23 — M2.5a.
- 2026-10-23 — M2.5b.
- 2026-10-23 — M2.6.
- 2026-10-23 — M2.7a.
- 2026-10-23 — M2.7b; M2 done.
- 2026-10-23 — M3.1.
- 2026-10-23 — M3.2.
- 2026-10-26 — M3.3.
- 2026-10-26 — M3.3b.
- 2026-10-26 — M3.3c.
- 2026-10-27 — M3.4.
- 2026-10-27 — M3.5.
- 2026-10-30 — M3.9.
- 2026-10-30 — M4.1.
- 2026-10-30 — M4.2.
- 2026-10-30 — M4.3.
- 2026-11-01 — M4.4.
- 2026-11-01 — M4.5a.
- 2026-11-01 — M4.5b part 1 (Autodiscover).
- 2026-11-01 — M4.5b part 2 (EWS calendars and contacts).
- 2026-11-01 — M4.5c (NTLM sign-in).
- 2026-11-03 — M5.1 (.msg import).
- 2026-11-03 — M5.1b (compressed-RTF bodies).
- 2026-11-03 — .msg HTML bodies read in the Internet code page.
- 2026-11-03 — M5.2 .pst mail (engine, CLI); zero-font-size HTML fix.
- 2026-11-03 — M5.2 .pst from the window; dropped files; folder.delete.
- 2026-11-03 — M5.2b .pst calendar and contacts.
- 2026-11-04 — M6.1 MCP server.
- 2026-11-04 — contacts in GNOME (EDS); Send/Receive period.
- 2026-11-05 — new look after Outlook for macOS; colour themes; hideable panes.
- 2026-10-22 — M2.1.
- 2026-10-22 — M2.2.
- 2026-10-22 — M2.3.
- 2026-10-22 — M2.4.
