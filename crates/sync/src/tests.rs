use std::io::{Read, Write};

use ibercorreo_core::{Message, RemoteFolder};

use super::*;

const A: AccountId = AccountId(0);

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
    let report = apply(&mut store, A, first_sync());
    assert_eq!((report.added, report.folders_added, report.remaining), (3, 1, 3), "{report:?}");
    assert_eq!(store.folders().len(), folders_before + 1, "INBOX and Sent Items reuse Inbox and Sent; Work is not selectable");
    let inbox = store.folder_by_role(A, FolderRole::Inbox).unwrap();
    assert_eq!(store.folder(inbox).unwrap().remote, Some(RemoteFolder { path: "INBOX".into(), uid_validity: 7 }));
    assert_eq!(store.folder_by_path(A, "Work/2026").map(|f| store.folder(f).unwrap().name.clone()).as_deref(), Some("Work/2026"));
    let one = by_subject(&store, "one");
    assert!(one.seen && one.folder == inbox);
    assert_eq!(one.remote, Some(RemoteRef { folder: inbox, uid: 1, seen: true, flagged: false }));
    // Applying the same batch again changes nothing.
    let again = apply(&mut store, A, first_sync());
    assert_eq!((again.added, again.folders_added), (0, 0));
}

#[test]
fn local_changes_become_pushes_and_known_uids() {
    let mut store = Store::new();
    apply(&mut store, A, first_sync());
    let inbox = store.folder_by_role(A, FolderRole::Inbox).unwrap();
    let sent_folder = store.folder_by_role(A, FolderRole::Sent).unwrap();
    let archive = store.folder_by_role(A, FolderRole::Archive).unwrap();
    let two = by_subject(&store, "two").id;
    store.message_mut(two).unwrap().seen = true;
    store.message_mut(two).unwrap().flagged = true;
    let one = by_subject(&store, "one").id;
    store.move_message(one, archive).unwrap(); // Archive has no server mailbox: nothing to send
    let sent_msg = by_subject(&store, "sent").id;
    store.remove_message(sent_msg).unwrap();

    let s = state(&store, A);
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
    assert!(state(&store, A).pushes.contains(&Push::Move { path: "INBOX".into(), uid: 1, to: "Sent Items".into() }));
    let _ = inbox;
}

#[test]
fn second_sync_updates_flags_removes_vanished_and_keeps_unsent_changes() {
    let mut store = Store::new();
    apply(&mut store, A, first_sync());
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
    let report = apply(&mut store, A, batch);
    assert_eq!((report.removed, report.updated), (1, 1));
    assert!(store.message(one).is_err());
    let m = store.message(two).unwrap();
    assert!(m.seen && m.flagged, "the local read survives, the server's flag arrives");
    assert_eq!(m.remote.map(|r| (r.seen, r.flagged)), Some((false, true)));
}

#[test]
fn uidvalidity_change_voids_held_messages() {
    let mut store = Store::new();
    apply(&mut store, A, first_sync());
    let inbox = store.folder_by_role(A, FolderRole::Inbox).unwrap();
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
    let report = apply(&mut store, A, batch);
    assert_eq!((report.removed, report.added), (2, 1));
    let subjects: Vec<_> = store.messages_in(inbox).iter().map(|m| m.subject.clone()).collect();
    assert_eq!(subjects.len(), 2);
    assert!(subjects.contains(&"local only".to_string()) && subjects.contains(&"renumbered".to_string()));
    assert_eq!(store.folder(inbox).unwrap().remote.as_ref().map(|r| r.uid_validity), Some(8));
}

#[test]
fn successful_pushes_are_recorded() {
    let mut store = Store::new();
    apply(&mut store, A, first_sync());
    let two = by_subject(&store, "two").id;
    store.message_mut(two).unwrap().seen = true;
    let sent_msg = by_subject(&store, "sent").id;
    store.remove_message(sent_msg).unwrap();
    let pushes = state(&store, A).pushes;
    let batch = Batch { pushed: pushes.into_iter().map(|p| (p, Ok(()))).collect(), ..Batch::default() };
    let report = apply(&mut store, A, batch);
    assert_eq!(report.pushed, 2);
    assert!(state(&store, A).pushes.is_empty());
    assert!(store.tombstones().is_empty());
}

#[test]
fn run_pushes_then_fetches() {
    let mut store = Store::new();
    apply(&mut store, A, first_sync());
    let two = by_subject(&store, "two").id;
    store.message_mut(two).unwrap().flagged = true;
    let s = state(&store, A);
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

mod sending {
    use super::super::outbox::{Outcome, Outgoing, Sent, send};
    use super::*;

    fn smtp(replies: &str) -> ibercorreo_smtp::Client<Script> {
        let s = Script { replies: std::io::Cursor::new(replies.replace('\n', "\r\n").into_bytes()), sent: Vec::new() };
        ibercorreo_smtp::Client::new(s, true, "h").unwrap()
    }

    fn mail(id: u64, to: &str) -> Outgoing {
        Outgoing { id: MessageId(id), from: "me@example.org".into(), to: vec![to.into()], raw: b"Subject: x\r\n\r\nhi\r\n".to_vec() }
    }

    #[test]
    fn outcomes_by_reply() {
        let mut c = smtp(
            "220 hi
250 hello
250 ok
250 ok
354 go
250 queued
250 ok
550 no such user
250 reset
451 busy
",
        );
        let results = send(&mut c, &[mail(1, "a@b.c"), mail(2, "nobody@b.c"), mail(3, "c@d.e"), mail(4, "f@g.h")]);
        let outcomes: Vec<_> = results.iter().map(|r: &Sent| r.outcome.clone()).collect();
        assert!(matches!(outcomes.first(), Some(Outcome::Sent { saved: false })));
        assert!(matches!(outcomes.get(1), Some(Outcome::Failed(e)) if e.contains("550")));
        assert!(matches!(outcomes.get(2), Some(Outcome::Later(e)) if e.contains("451")));
        // The script ends: the connection is lost, and what is left waits for the next try.
        assert!(matches!(outcomes.get(3), Some(Outcome::Later(_))));
        assert_eq!(results.len(), 4);
    }

    #[test]
    fn saved_copies_go_to_sent() {
        let mut results =
            vec![Sent { id: MessageId(1), outcome: Outcome::Sent { saved: false } }, Sent { id: MessageId(2), outcome: Outcome::Later("x".into()) }];
        let mut c = client("* OK [CAPABILITY IMAP4rev1 LITERAL+] hi\na1 OK appended\n");
        super::super::outbox::save_sent(&mut c, "Sent", &[mail(1, "a@b.c"), mail(2, "c@d.e")], &mut results);
        assert!(matches!(results.first().map(|r| &r.outcome), Some(Outcome::Sent { saved: true })));
        assert_eq!(sent(c), "a1 APPEND Sent (\\Seen) {18+}\nSubject: x\n\nhi\n\n");
    }

    #[test]
    fn a_server_that_files_sent_mail_itself_gets_no_copy() {
        let mut results =
            vec![Sent { id: MessageId(1), outcome: Outcome::Sent { saved: false } }, Sent { id: MessageId(2), outcome: Outcome::Later("x".into()) }];
        super::super::outbox::server_saved(&mut results);
        assert!(matches!(results.first().map(|r| &r.outcome), Some(Outcome::Sent { saved: true })), "its copy comes with the next sync");
        assert!(matches!(results.get(1).map(|r| &r.outcome), Some(Outcome::Later(_))), "unsent mail stays queued");
        let gmail = ibercorreo_core::mail::OAuthProvider::for_host("smtp.gmail.com");
        assert!(gmail.is_some_and(|p| p.saves_sent));
    }
}

#[test]
fn two_accounts_sync_into_their_own_folders() {
    let mut store = Store::new();
    let account = |email: &str| ibercorreo_core::Account {
        id: AccountId(0),
        name: email.into(),
        address: ibercorreo_core::Address::parse(email).unwrap(),
        kind: ibercorreo_core::mail::AccountKind::Local,
        caldav: String::new(),
    };
    let a = store.add_account(account("ana@example.org")).unwrap();
    let b = store.add_account(account("bo@example.net")).unwrap();
    apply(&mut store, a, first_sync());
    let report = apply(&mut store, b, first_sync());
    assert_eq!((report.added, report.folders_added), (3, 1), "the same mailboxes on the second server are new: {report:?}");
    let (ia, ib) = (store.folder_by_role(a, FolderRole::Inbox).unwrap(), store.folder_by_role(b, FolderRole::Inbox).unwrap());
    assert_eq!(store.messages_in(ia).len(), 2);
    assert_eq!(store.messages_in(ib).len(), 2);
    assert!(store.folder_by_path(a, "Work/2026") != store.folder_by_path(b, "Work/2026"));
    // A change in one account is pushed only to its own server.
    let mine = store.messages_in(ib).first().map(|m| m.id).unwrap();
    store.message_mut(mine).unwrap().flagged = true;
    assert!(state(&store, a).pushes.is_empty());
    assert_eq!(state(&store, b).pushes.len(), 1);
    assert!(state(&store, b).known.iter().all(|k| store.folder_by_path(b, &k.path).is_some()));
    // A reset of one account's Inbox leaves the other's mail alone.
    let batch = Batch {
        mailboxes: first_sync().mailboxes,
        folders: vec![FolderSync { path: "INBOX".into(), uid_validity: 8, reset: true, ..FolderSync::default() }],
        ..Batch::default()
    };
    apply(&mut store, a, batch);
    assert!(store.messages_in(ia).is_empty());
    assert_eq!(store.messages_in(ib).len(), 2);
}
