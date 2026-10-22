use std::io::{Read, Write};

use ibercorreo_core::{Message, RemoteFolder};

use super::*;

struct Script {
    replies: std::io::Cursor<Vec<u8>>,
    sent: Vec<u8>,
}

impl Read for Script {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.replies.read(buf)
    }
}

impl Write for Script {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.sent.extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn client(replies: &str) -> Client<Script> {
    let script = Script { replies: std::io::Cursor::new(replies.replace('\n', "\r\n").into_bytes()), sent: Vec::new() };
    Client::new(script, true).unwrap()
}

fn sent(c: Client<Script>) -> String {
    String::from_utf8(c.into_stream().sent).unwrap().replace("\r\n", "\n")
}

fn mailbox(raw: &str, attrs: &[&str]) -> Mailbox {
    Mailbox { raw: raw.into(), name: raw.into(), delimiter: Some('/'), attributes: attrs.iter().map(|s| s.to_string()).collect() }
}

fn raw(subject: &str) -> Vec<u8> {
    format!("From: Ana <ana@example.org>\r\nSubject: {subject}\r\nDate: Thu, 22 Oct 2026 09:00:00 +0000\r\n\r\nbody of {subject}\r\n").into_bytes()
}

fn first_sync() -> Batch {
    Batch {
        mailboxes: vec![mailbox("INBOX", &[]), mailbox("Sent Items", &["\\Sent"]), mailbox("Work/2026", &[]), mailbox("Work", &["\\Noselect"])],
        folders: vec![
            FolderSync {
                path: "INBOX".into(),
                uid_validity: 7,
                new: vec![
                    NewMessage { uid: 1, flags: vec!["\\Seen".into()], raw: raw("one") },
                    NewMessage { uid: 2, flags: vec![], raw: raw("two") },
                ],
                remaining: 3,
                ..FolderSync::default()
            },
            FolderSync {
                path: "Sent Items".into(),
                uid_validity: 9,
                new: vec![NewMessage { uid: 5, flags: vec!["\\Seen".into()], raw: raw("sent") }],
                ..FolderSync::default()
            },
            FolderSync { path: "Work/2026".into(), uid_validity: 1, ..FolderSync::default() },
        ],
        ..Batch::default()
    }
}

fn by_subject(store: &Store, subject: &str) -> Message {
    store.all_messages().iter().find(|m| m.subject == subject).cloned().unwrap()
}

#[test]
fn first_sync_links_standard_folders_and_adds_mail() {
    let mut store = Store::new();
    let folders_before = store.folders().len();
    let report = apply(&mut store, first_sync());
    assert_eq!((report.added, report.folders_added, report.remaining), (3, 1, 3), "{report:?}");
    assert_eq!(store.folders().len(), folders_before + 1, "INBOX and Sent Items reuse Inbox and Sent; Work is not selectable");
    let inbox = store.folder_by_role(FolderRole::Inbox).unwrap();
    assert_eq!(store.folder(inbox).unwrap().remote, Some(RemoteFolder { path: "INBOX".into(), uid_validity: 7 }));
    assert_eq!(store.folder_by_path("Work/2026").map(|f| store.folder(f).unwrap().name.clone()).as_deref(), Some("Work/2026"));
    let one = by_subject(&store, "one");
    assert!(one.seen && one.folder == inbox);
    assert_eq!(one.remote, Some(RemoteRef { folder: inbox, uid: 1, seen: true, flagged: false }));
    // Applying the same batch again changes nothing.
    let again = apply(&mut store, first_sync());
    assert_eq!((again.added, again.folders_added), (0, 0));
}

#[test]
fn local_changes_become_pushes_and_known_uids() {
    let mut store = Store::new();
    apply(&mut store, first_sync());
    let inbox = store.folder_by_role(FolderRole::Inbox).unwrap();
    let sent_folder = store.folder_by_role(FolderRole::Sent).unwrap();
    let archive = store.folder_by_role(FolderRole::Archive).unwrap();
    let two = by_subject(&store, "two").id;
    store.message_mut(two).unwrap().seen = true;
    store.message_mut(two).unwrap().flagged = true;
    let one = by_subject(&store, "one").id;
    store.move_message(one, archive).unwrap(); // Archive has no server mailbox: nothing to send
    let sent_msg = by_subject(&store, "sent").id;
    store.remove_message(sent_msg).unwrap();

    let s = state(&store);
    assert_eq!(
        s.pushes,
        [Push::Flags { path: "INBOX".into(), uid: 2, seen: true, flagged: true }, Push::Expunge { path: "Sent Items".into(), uid: 5 },]
    );
    let inbox_known = s.known.iter().find(|k| k.path == "INBOX").unwrap();
    assert_eq!(inbox_known.uids.iter().copied().collect::<Vec<_>>(), [1, 2], "a moved message is still held in its server mailbox");
    let sent_known = s.known.iter().find(|k| k.path == "Sent Items").unwrap();
    assert!(sent_known.uids.contains(&5), "a deleted message stays known until the server deletes it, so it is not downloaded again");

    // Moving to a synced folder is a push.
    store.move_message(one, sent_folder).unwrap();
    assert!(state(&store).pushes.contains(&Push::Move { path: "INBOX".into(), uid: 1, to: "Sent Items".into() }));
    let _ = inbox;
}

#[test]
fn second_sync_updates_flags_removes_vanished_and_keeps_unsent_changes() {
    let mut store = Store::new();
    apply(&mut store, first_sync());
    let one = by_subject(&store, "one").id;
    let two = by_subject(&store, "two").id;
    // Read here but not yet sent: the server's older "unread" must not undo it.
    store.message_mut(two).unwrap().seen = true;
    let batch = Batch {
        mailboxes: first_sync().mailboxes,
        folders: vec![FolderSync {
            path: "INBOX".into(),
            uid_validity: 7,
            flags: vec![(2, vec!["\\Flagged".into()])],
            vanished: vec![1],
            ..FolderSync::default()
        }],
        ..Batch::default()
    };
    let report = apply(&mut store, batch);
    assert_eq!((report.removed, report.updated), (1, 1));
    assert!(store.message(one).is_err());
    let m = store.message(two).unwrap();
    assert!(m.seen && m.flagged, "the local read survives, the server's flag arrives");
    assert_eq!(m.remote.map(|r| (r.seen, r.flagged)), Some((false, true)));
}

#[test]
fn uidvalidity_change_voids_held_messages() {
    let mut store = Store::new();
    apply(&mut store, first_sync());
    let inbox = store.folder_by_role(FolderRole::Inbox).unwrap();
    let mut local = Message::new(MessageId(0), inbox);
    local.set_subject("local only");
    store.add_message(local).unwrap();
    let batch = Batch {
        mailboxes: first_sync().mailboxes,
        folders: vec![FolderSync {
            path: "INBOX".into(),
            uid_validity: 8,
            reset: true,
            new: vec![NewMessage { uid: 1, flags: vec![], raw: raw("renumbered") }],
            ..FolderSync::default()
        }],
        ..Batch::default()
    };
    let report = apply(&mut store, batch);
    assert_eq!((report.removed, report.added), (2, 1));
    let subjects: Vec<_> = store.messages_in(inbox).iter().map(|m| m.subject.clone()).collect();
    assert_eq!(subjects.len(), 2);
    assert!(subjects.contains(&"local only".to_string()) && subjects.contains(&"renumbered".to_string()));
    assert_eq!(store.folder(inbox).unwrap().remote.as_ref().map(|r| r.uid_validity), Some(8));
}

#[test]
fn successful_pushes_are_recorded() {
    let mut store = Store::new();
    apply(&mut store, first_sync());
    let two = by_subject(&store, "two").id;
    store.message_mut(two).unwrap().seen = true;
    let sent_msg = by_subject(&store, "sent").id;
    store.remove_message(sent_msg).unwrap();
    let pushes = state(&store).pushes;
    let batch = Batch { pushed: pushes.into_iter().map(|p| (p, Ok(()))).collect(), ..Batch::default() };
    let report = apply(&mut store, batch);
    assert_eq!(report.pushed, 2);
    assert!(state(&store).pushes.is_empty());
    assert!(store.tombstones().is_empty());
}

#[test]
fn run_pushes_then_fetches() {
    let mut store = Store::new();
    apply(&mut store, first_sync());
    let two = by_subject(&store, "two").id;
    store.message_mut(two).unwrap().flagged = true;
    let s = state(&store);
    let mut c = client(
        "* OK [CAPABILITY IMAP4rev1 UIDPLUS] hi
a1 OK [READ-WRITE] selected
a2 OK stored
a3 OK stored
* LIST () \"/\" INBOX
* LIST (\\Noselect) \"/\" Work
a4 OK listed
* 4 EXISTS
* OK [UIDVALIDITY 7] ok
a5 OK [READ-ONLY] examined
* SEARCH 2 3 4 10
a6 OK searched
* 1 FETCH (UID 2 FLAGS (\\Flagged))
a7 OK fetched
* 2 FETCH (UID 4 FLAGS () BODY[] {4}
four)
* 3 FETCH (UID 10 FLAGS (\\Seen) BODY[] {3}
ten)
a8 OK fetched
",
    );
    let limits = Limits { new_per_folder: 2, ..Limits::default() };
    let batch = run(&mut c, &s, &limits).unwrap();
    assert_eq!(batch.pushed.len(), 1);
    assert!(batch.errors.is_empty(), "{:?}", batch.errors);
    let inbox = batch.folders.first().unwrap();
    assert_eq!(inbox.vanished, [1]);
    assert_eq!(inbox.remaining, 1, "UID 3 waits for the next sync; the newest come first");
    assert_eq!(inbox.new.iter().map(|n| n.uid).collect::<Vec<_>>(), [4, 10]);
    assert_eq!(
        sent(c),
        "a1 SELECT INBOX
a2 UID STORE 2 +FLAGS.SILENT (\\Flagged)
a3 UID STORE 2 -FLAGS.SILENT (\\Seen)
a4 LIST \"\" \"*\"
a5 EXAMINE INBOX
a6 UID SEARCH UID 1:*
a7 UID FETCH 2 (UID FLAGS RFC822.SIZE INTERNALDATE)
a8 UID FETCH 4,10 (UID FLAGS RFC822.SIZE INTERNALDATE BODY.PEEK[])
"
    );
}

#[test]
fn one_failing_mailbox_does_not_stop_the_others() {
    let s = State::default();
    let mut c = client(
        "* OK [CAPABILITY IMAP4rev1] hi
* LIST () \"/\" Broken
* LIST () \"/\" INBOX
a1 OK listed
a2 NO no such mailbox
* 0 EXISTS
* OK [UIDVALIDITY 3] ok
a3 OK examined
",
    );
    let batch = run(&mut c, &s, &Limits::default()).unwrap();
    assert_eq!(batch.errors.len(), 1);
    assert_eq!(batch.folders.len(), 1);
    // A connection lost mid-sync is an error for the whole sync.
    let mut c = client("* OK [CAPABILITY IMAP4rev1] hi\n* LIST () \"/\" INBOX\na1 OK listed\n");
    assert_eq!(run(&mut c, &s, &Limits::default()), Err(Error::Closed));
}
