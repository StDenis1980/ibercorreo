# Tinta Correo — ranked gaps

Status: 2026-11-24 · Pick from the top.

1. **Mail accounts: the basics only.** Account settings, Send/Receive (background, at start,
   every 5 minutes, F9) and the password in the system credential store work, checked against
   Dovecot and Postfix. Find settings (autoconfig files, the public database, the usual server
   names) and IDLE push on the Inbox work (M3.1), and so does OAuth sign-in for Gmail and
   Microsoft 365 (M3.2), once a build carries registered client IDs (`docs/oauth.md`: Google's
   needs verification and a security assessment before anyone but test users can sign in).
   Several accounts sit side by side, each with its own folders, Send/Receive and IDLE watch
   (M3.3); choosing Google or Microsoft (or typing their address) fills in the servers; a folder
   with more than 500 new messages keeps downloading until it is complete. Calendars and contacts
   sync with one account's server at a time. The folder pane has Favorites, folding accounts and
   a tree of subfolders, as Outlook's does, its folds kept after a restart; each account has its own signature, compose has Bcc, and the
   status bar shows sync progress; All Inboxes shows every account's Inbox together (M3.4). Missing: moving mail between
   accounts, reordering Favorites by dragging, DNS SRV (RFC 6186) and MX lookups (no DNS
   resolver yet), a separate CardDAV/CalDAV address in Find settings, OAuth for Google's
   calendars and contacts, other OAuth providers. Sent copies are saved to the server's Sent
   except on Gmail, which files them itself (Microsoft 365 may too: unchecked, an account setting
   will cover others); a message is refused as a whole if any recipient is.
   Known sync limits: a message moved between server folders is downloaded again (no COPYUID
   remapping); a folder whose mailbox the server no longer lists is unlinked and kept here
   with its mail (a standard one is re-linked to the server's own); whole messages are
   downloaded (no header-first fetch).
   Microsoft 365 work accounts without IMAP sync mail over Microsoft Graph (M4.4, an account
   setting). Missing there: delta queries (every sync lists each folder's ids), push
   (subscriptions; the 5-minute sync instead), sending messages over 4 MB (an upload session),
   a moved message keeping its download (immutable ids would allow it), checking against a real
   tenant with IMAP turned off.
   On-premises Exchange syncs mail over EWS (M4.5a, an account setting). Sign-in is Basic or NTLM
   (M4.5c, bound to the certificate for Extended Protection). Missing there: Kerberos sign-in
   (servers that offer only Negotiate without NTLM), Autodiscover's DNS SRV and
   plain-HTTP redirect steps (the two HTTPS places are asked), meeting invitations sent from
   here over EWS (changes go with SendToNone), keeping an item's id when it is changed here (it
   is replaced), `SyncFolderItems` deltas and push
   (streaming notifications), checking against a real server.
2. **HTML mail, the first pass.** HTML is read into safe formatted text (headings, lists, quotes,
   bold and italic, links with their real address on hover and a ⚠ on phishing-shaped ones);
   in plain-text mail web and e-mail addresses are links too; a whole message can be selected
   and copied, past the pane's edge, with Ctrl+A or Copy all;
   scripts, forms, styles and hidden preview text are dropped and remote images are never loaded.
   Pictures the message carries itself (`cid:`, RFC 2392) are drawn in their place, no wider than
   the pane, and get no attachment chip (their context menu saves them); mail that arrived before
   attachment contents were kept still shows their placeholders.
   Pictures on the web are downloaded only when the person asks: for one message, or always for
   its sender (`message.loadPictures`), over HTTPS only, never from this computer or its local
   network, and never tracking pixels (M6.2d).
   Tables of data are drawn as tables (layout tables flow as paragraphs); the sender's text
   colours, highlights and clearly larger or smaller sizes are kept where they stay readable on
   the theme (M6.2e), with merged cells and cell backgrounds; a link whose text names another
   address asks before it opens (M3.4). Missing: the sender's font families and a true bold weight (the interface font has one weight, so bold is shown in
   the strong text colour).
3. **Credentials**: passwords live in the system credential store; where none runs (a bare Linux
   session) they last only until the app closes. No OAuth yet (M3).
4. **Recurring events** expand (`DAILY`…`YEARLY` with `BYDAY`, `BYMONTHDAY`, `BYMONTH`, `BYSETPOS`,
   `COUNT`, `UNTIL`; `EXDATE`; changed occurrences by `RECURRENCE-ID`). Missing: editing one
   occurrence in the window (only deleting it), sub-daily and `BYWEEKNO`/`BYYEARDAY` rules (shown
   once), `RDATE`, and exceptions matched by day only (one occurrence a day at most).
5. **Time zones** work: events in a zone (IANA, Windows names as Outlook writes them, vendor
   ids) and in UTC show in the zone chosen (default: the computer's), repeating events follow
   summer time in their own zone, and mail dates are kept in UTC and shown locally. Missing:
   reading `VTIMEZONE` (a zone only Outlook's own definition describes, like "Customized Time
   Zone", shows as written), writing `VTIMEZONE` for repeating events (one-off ones are exported in
   UTC; the IANA id is enough for Google, Apple and Thunderbird), a second zone column in the day
   view, and choosing an appointment's zone in the form (it takes the calendar's).
6. **Undo** snapshots the whole store (50 steps; the messages are shared between the steps, so
   only the ones a command changed are copied), and a sync clears the history (stepping back past
   a sync would resurrect stale server state). Replace with operation-level undo.
7. **Store**: `store.json` with the messages in shard files beside it (`store.mail/`, a thousand
   ids each); a background save rewrites only the shards that changed, and no file grows with the
   mailbox. Message text stays on disk in each shard's content file and is read when shown
   (M3.9), so only headers sit in memory. The search index is kept on disk too (`store.index/`,
   2026-11-17): a start reads only a small sample of its words, the messages changed since it was
   written are indexed in memory, and a save writes a new one once more than 2,000 are. With
   67,000 messages a fresh start takes about 42 MB in place of 400 MB, and search is ready at
   once in place of after 11 seconds. Next: the first save after an upgrade writes the index (a
   few seconds on a background thread, longer without a window).
8. **Attachments**: the contents of mail that arrives are kept beside the mailbox (2026-11-13), and
   an assistant can save one through MCP (`save_attachment`); the reading pane has Open and Save on
   each attachment (Save to the Downloads folder under a free name; Open as a copy, never for a
   program or script) (2026-11-14). Files are attached to a draft with Attach file beside Send, by
   dropping them on the window while a draft is open, with `message.attach` or with MCP's
   `attach_file` (only from `--automation-read-root` folders); each has a × to take it off, they
   are kept with the mailbox like those that arrive, and they go out over SMTP, Microsoft Graph
   and EWS as `multipart/mixed`. A forward carries the original's kept attachments (2026-11-17).
   A draft carries at most 100 files and about 44 MiB in all; Microsoft Graph sends at most 4 MB
   per message. The files of `.pst`/`.msg` imports are kept and go out in the mbox export
   (2026-11-24). Send asks first when the words of a draft say a file is attached but it has none
   (in the interface's 13 languages; quoted text and a reply's subject do not count), and
   `message.read` says so as `forgotAttachment` (2026-11-24). Paste beside Attach file attaches
   the files copied in a file manager, or else the picture on the clipboard as a PNG named after
   the time; Ctrl+V in a draft attaches copied files too (2026-11-24). A picture alone reaches the
   window with no Ctrl+V to see (egui's window layer only hands on text), so it takes the button.
   Missing: choosing where to save, downloading the attachments of mail that arrived before,
   pictures inside the text of a message written here, the Tinta PDF, Texto and Calculo
   hand-off.
9. **Localization**: the interface is English only; Ukrainian, Spanish and Portuguese planned.
10. **Calendar views**: day, work week, week and month work. Appointments drag to another time or
    day and their bottom edge stretches them; the work hours are chosen on the ribbon (the grid
    opens an hour before they start and shades the rest), with the working days (the Work Week
    view) and the first day of the week; all-day events drag in the strip above the hours, and any
    appointment that does not repeat drags to another day in the month. Missing: dragging a
    repeating appointment (it needs the "this one or the series" question).
11. **Reminders** fire in the window (Dismiss, Snooze, Dismiss All; dismissals saved, missed ones
    kept two days) and travel in `.ics` as `VALARM`. Missing: a system notification and a sound
    when the window is in the background or closed (needs a tray/background process), a
    per-person default other than 15 minutes, and reminders on mail (follow-up flags).
12. **CalDAV sync works both ways (M2.5).** Calendars are found from a server, principal or
    calendar address; at Send/Receive changes made here go up (each only if the server's copy is
    unchanged; otherwise the server's copy wins and comes down) and changes there come down
    (only what changed). Events written back keep what Tinta Correo does not read. Checked
    against Radicale. Missing: telling the person which change lost a conflict (only a count in
    the status bar), `sync-collection` (RFC 6578) instead of listing every ETag, a separate
    CalDAV password, tasks (`VTODO`), creating, renaming or hiding a calendar, moving an
    appointment to another calendar, and checking against Nextcloud, Google and iCloud.
    Microsoft accounts' Outlook calendars sync over Microsoft Graph instead (M4.2), both ways,
    with series, their moved and deleted occurrences, and reminders. Missing there: delta
    queries (every sync lists each calendar's events), occurrences changed outside the window
    (a year back, two ahead), attendees and organizing, read-only calendars marked as such, and
    removing an account's Outlook calendars with the account.
13. **Meeting invitations, as an attendee (M2.6).** An invitation by mail (iMIP) shows a strip with
    Accept, Tentative and Decline; the meeting goes into (or out of) the calendar, an update moves
    the same event, and the answer (`METHOD:REPLY`) goes to the organizer, sent now, edited first
    or not at all. A cancellation can be removed, a whole series or one day. Checked end to end:
    an invitation sent through Postfix, accepted into a Radicale calendar, the answer delivered.
    Missing: sending invitations (organizing a meeting: attendees in the appointment form,
    `REQUEST`/`CANCEL` out, applying `REPLY`s to the attendee list), showing tentative meetings
    differently, `COUNTER` (propose a new time) and `DELEGATED`, conflicts with the calendar shown
    in the strip, the invitation's attendees and organizer kept on the event (a server calendar gets
    the meeting without them), server-side scheduling (CalDAV `schedule-inbox`, RFC 6638), and
    moving an already accepted meeting to the calendar chosen when answering again.
14. **Contacts (M2.7).** Cards with several addresses and phones, organization, job
    title, birthday and notes, edited in People; `.vcf` import (vCard 2.1 from Outlook, 3.0, 4.0)
    and export (3.0), keeping every property not shown (postal addresses, photos, web sites) so a
    round trip loses nothing. Address books synced both ways with a CardDAV server (M2.7b, the
    same address as the calendars), checked against Radicale. Microsoft accounts' Outlook
    contacts (the Contacts folder and its subfolders) sync both ways over Microsoft Graph (M4.3);
    Outlook keeps three mail addresses and one mobile a contact (more is reported when sent), and
    postal addresses and photos are not read from Graph yet. Missing: checking against
    Nextcloud, Google (needs OAuth and its own CardDAV address, M3) and iCloud; a separate
    CardDAV address when it differs from the CalDAV one; creating, renaming and deleting address
    books (on the server too); `sync-collection` (RFC 6578) instead of a full listing on every
    change; cards written back as 3.0 even when the server held 4.0; contact groups and lists, photos
    shown, postal addresses and web sites edited, birthdays in the calendar, and an export button in the
    window (export is a command). A multi-part organization (`ORG:Company;Department`) is kept
    as one text ("Company, Department").
    Add to contacts sits beside a message's sender (and on its context menu, with Copy address),
    and a sender already there is not added twice (`contact.addSender`); To, Cc and Bcc offer
    addresses while they are typed, contacts first, then the people written to (`contact.suggest`,
    2026-11-24). Missing there: contact groups offered as one recipient, and forgetting an
    address offered.
15. **Search (M3.4).** An in-memory full-text index (words from their start, accents and case
    ignored) with Outlook-style queries (phrases, `-word`, `OR`, field prefixes, `has:`, `is:`,
    dates) and the All mailboxes / Current mailbox / Current folder scope. The index is kept
    on disk (gap 7); only the messages changed since it was written are indexed at start or
    after a Send/Receive. Missing: searching
    attachment contents, the server's search (IMAP `SEARCH`) for mail not downloaded, highlighting
    the words found, a Search ribbon tab with filter buttons, search history and saved searches
    (search folders), relevance order (results are newest first) and `size:` (the size of a
    message is not kept). Date words (`received:yesterday`, `"last week"`, `"минулого місяця"`,
    `"semana pasada"`…) and `in:` a folder by name or role came 2026-11-24; in rules they are
    still words.16. **Rules (M3.5).** Client-side rules on new Inbox mail and on demand: a search-syntax
    condition; mark read, flag, move, delete, stop. Missing: server-side rules (Sieve,
    ManageSieve RFC 5804; Exchange inbox rules with M4) so mail is sorted while the app is closed,
    forward/redirect and auto-reply actions, categories, "except if" conditions as their own
    field (`-word` covers simple cases), rules on sent mail, rules on mail arriving in folders
    other than the Inbox, import of Outlook rules (`.rwz`), and moves pushed to the server at
    once (they go at the next Send/Receive).
17. **Moving from Outlook (M5).** A `.msg` file opens into the Inbox (M5.1): sender, recipients,
    subject, date, read state, plain and HTML body (also when Outlook kept it only as compressed
    RTF, M5.1b), and the attachment list. The mail of a `.pst` archive imports into local folders
    named after the archive's (M5.2), opened with the app or dropped on the window and read in
    the background; an imported folder can be deleted. Its calendars and contacts become local
    calendars and address books (M5.2b), with recurring series and their exceptions. Everything
    exports to mbox, iCalendar and vCard (M5.3, engine and CLI), and `docs/outlook-migration.md`
    guides the move. Attachment files come with the mail of a `.pst` or `.msg` and go out in the
    mbox export (2026-11-24; written to disk as the archive is read). Missing: the
    tasks, notes and contact groups of a `.pst`, meeting attendees, Hijri-calendar series, `.ost`
    files, the formatting of mail written as RTF (its text is shown), contacts, appointments and
    tasks saved as `.msg`, opening an attached message, Outlook rules (`.rwz`), a File > Open
    dialog, a Linux file type for `.pst`, and writing `.pst`.
18. **Colour categories (M6.2f).** Categories with a name and one of eight colours are made,
    renamed, recoloured and deleted in the Categories window or with `category.*`; Categorize
    (toolbar, a message's context menu, `message.categorize`, the MCP tool `categorize_message`)
    gives them to mail, the list shows a dot per category and the reading pane their names, and
    search finds them with `category:`. Missing: keeping them on the server (IMAP keywords,
    Exchange and Microsoft 365 categories) so other devices see them, importing Outlook's
    categories from `.pst` and `.msg`, a rule action that categorizes, a quick-click category,
    and categories on appointments and contacts.
19. **Conversations (M6.2g).** View › Conversations (the button beside the folder's name,
    `view.conversations`) groups a folder's mail by subject once `Re:`, `Fwd:` and their
    translations are taken off, as Outlook does: one row per conversation with its newest
    message and a count, opened with its arrow or a click; `message.conversation` (MCP
    `get_conversation`) lists a conversation across the account's folders. Missing: grouping by
    `Message-ID`/`References` (headers not kept yet), the account's Sent mail shown inside a
    folder's conversations, a reading pane that shows the whole conversation, and actions on a
    whole conversation (delete, move, categorize).
20. **Mail header first (M6.2h).** Account settings › Download whole messages
    (`account.fullMail`, 0–120 months; 0, the default, downloads all mail whole): mail that
    arrived before then comes as its header (over IMAP `BODY.PEEK[HEADER]`; over Exchange (EWS)
    and Microsoft Graph a header built from the message's fields, M6.2i), so From, To, the
    subject, the date and the flags are there and search finds them, and its text and
    attachments come when it is opened (`message.download`, which starts a Send/Receive; on its
    own the first time the message is opened). Missing: a size limit (Outlook's "download only headers
    for messages over N KB"); dropping the text of old mail already downloaded to free space;
    and search inside the text of mail not downloaded (the server's `SEARCH BODY`).

## Revision history
- 2026-10-22 — created with M0.
- 2026-10-22 — gap 1 narrowed: IMAP protocol core.
- 2026-10-22 — gap 1 narrowed: TLS connections.
- 2026-10-22 — gap 1 narrowed: two-way sync (CLI); gap 6 notes undo is cleared by sync.
- 2026-10-22 — gap 1 narrowed: SMTP sending (CLI).
- 2026-10-22 — gap 1 narrowed: accounts in the window (M1.5); gap 3 closed (keychain).
- 2026-10-22 — gap 2 narrowed: safe HTML display (M1.6).
- 2026-10-22 — gap 4 narrowed: recurrence (M2.1).
- 2026-10-22 — gap 10 narrowed: day and week views (M2.2).
- 2026-10-22 — gap 5 narrowed: time zones (M2.3).
- 2026-10-22 — gap 11 added: reminders (M2.4).
- 2026-10-23 — gap 12 added: CalDAV pull (M2.5a).
- 2026-10-23 — gap 12 narrowed: CalDAV push (M2.5b).
- 2026-10-23 — gap 13 added: meeting invitations (M2.6); gap 10 no longer lists them.
- 2026-10-23 — gap 14 added: contacts and vCard (M2.7a).
- 2026-10-23 — gap 14 narrowed: CardDAV sync (M2.7b).
- 2026-10-23 — gap 1 narrowed: autoconfiguration and IDLE (M3.1).
- 2026-10-23 — gap 1 narrowed: OAuth sign-in for Gmail and Microsoft 365 (M3.2).
- 2026-10-26 — gap 1 narrowed: several accounts, servers filled in for Google and Microsoft, downloads past 500 (M3.3).
- 2026-10-26 — gap 1 narrowed: Favorites and folding accounts in the folder pane (M3.3b).
- 2026-10-26 — gap 1 narrowed: signatures, Bcc and a sync progress bar (M3.3c).
- 2026-10-27 — gap 15 added: search (M3.4).
- 2026-10-27 — gap 16 added: rules (M3.5).
- 2026-10-28 — gaps 6, 7 and 15 updated: big mailboxes (shared messages, background save and index).
- 2026-10-28 — gap 7 narrowed: messages saved in shards (M3.7).
- 2026-10-28 — gap 15: compact search index (M3.8).
- 2026-10-30 — gap 7 narrowed: message text kept on disk (M3.9).
- 2026-10-30 — gap 12: Outlook calendars over Microsoft Graph (M4.2).
- 2026-10-30 — gap 14: Outlook contacts over Microsoft Graph (M4.3).
- 2026-11-01 — gap 1: mail over Microsoft Graph for work accounts without IMAP (M4.4).
- 2026-11-01 — gap 1: mail over EWS for on-premises Exchange (M4.5a).
- 2026-11-01 — gap 1: Exchange Autodiscover (M4.5b).
- 2026-11-01 — gap 1: Exchange calendars and contacts over EWS (M4.5b).
- 2026-11-01 — gap 1: NTLM sign-in (M4.5c).
- 2026-11-03 — gap 17 added: moving from Outlook; `.msg` import (M5.1).
- 2026-11-03 — gap 17: compressed-RTF bodies (M5.1b).
- 2026-11-03 — gap 17: `.pst` mail (M5.2, engine and CLI).
- 2026-11-03 — gap 17: `.pst` from the window, dropped files.
- 2026-11-03 — gap 17: `.pst` calendar and contacts.
- 2026-11-06 — gap 1: folders the server no longer lists are unlinked; standard folders re-link.
- 2026-11-12 — gap 17: export to mbox/iCalendar/vCard, migration guide.
- 2026-11-12 — gaps 1 and 2 narrowed (M6.2): folder tree with folds kept; plain-text links; whole-message selection.
- 2026-11-13 — gap 17: the Export everything button.
- 2026-11-13 — gap 8: attachment contents kept; `save_attachment` over MCP.
- 2026-11-17 — gaps 7 and 15: the search index kept on disk.
- 2026-11-24 — gaps 1 and 2 narrowed (M3.4): All Inboxes, deceptive links, merged cells; gap 17: `.pst`/`.msg` attachment files kept and exported.
- 2026-11-24 — gap 8: the forgotten-attachment reminder.
- 2026-11-24 — gap 15: date words and `in:` in search.
- 2026-11-24 — gap 10: appointments drag to move and stretch.
- 2026-11-24 — gap 10: work hours.
- 2026-11-24 — gap 10: working days and the first day of the week.
- 2026-11-24 — gap 10: all-day events and month chips drag.
