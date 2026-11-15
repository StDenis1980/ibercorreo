//! Conversations in the message list (Tinta): with View › Conversations on, the mail of a folder
//! that shares a subject (once `Re:`, `Fwd:` and the like are taken off) is one row, its newest
//! message, with how many there are; the row opens to show the others under it. Grouping is the
//! engine's ([`ibercorreo_engine::conversations::group`]); this keeps the rows of the list.

use std::collections::HashSet;

use ibercorreo_core::MessageId;
use ibercorreo_engine::Session;

pub(crate) const SHOW: &str = "Show as conversations";
pub(crate) const HIDE: &str = "Show as single messages";

/// The strings this module shows, for the translation test.
#[cfg(test)]
const STRINGS: &[&str] = &[SHOW, HIDE, "Conversations"];

/// What a row of the list is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    /// A message alone (or the list is not grouped).
    One,
    /// The newest message of a conversation of `count`, open or not; `unread` when any of its
    /// messages is.
    Head { count: usize, open: bool, unread: bool, key: MessageId },
    /// An older message of an open conversation.
    Child,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Row {
    pub(crate) id: MessageId,
    pub(crate) kind: Kind,
}

/// The rows of the list, kept until the messages or what is open change.
#[derive(Default)]
pub(crate) struct Conversations {
    /// Open conversations, by their oldest message (which stays as replies come).
    open: HashSet<MessageId>,
    /// The messages and how many were unread when the rows were built, and the rows.
    cached: Option<(Vec<MessageId>, usize, Vec<Row>)>,
}

impl Conversations {
    /// The rows for `ids` (the folder's messages, newest first), grouped.
    pub(crate) fn rows(&mut self, session: &Session, ids: &[MessageId]) -> &[Row] {
        let unread_now = ids.iter().filter(|id| session.store.message(**id).is_ok_and(|m| !m.seen)).count();
        let fresh = self.cached.as_ref().is_some_and(|(at, unread, _)| at.as_slice() == ids && *unread == unread_now);
        if !fresh {
            let messages: Vec<_> = ids.iter().filter_map(|id| session.store.message(*id).ok()).collect();
            let mut rows = Vec::with_capacity(ids.len());
            for group in ibercorreo_engine::conversations::group(&messages) {
                let (Some(&first), Some(&key)) = (group.first(), group.last()) else { continue };
                if group.len() == 1 {
                    rows.push(Row { id: first, kind: Kind::One });
                    continue;
                }
                let unread = group.iter().any(|id| session.store.message(*id).is_ok_and(|m| !m.seen));
                let open = self.open.contains(&key);
                rows.push(Row { id: first, kind: Kind::Head { count: group.len(), open, unread, key } });
                if open {
                    rows.extend(group.iter().skip(1).map(|&id| Row { id, kind: Kind::Child }));
                }
            }
            self.cached = Some((ids.to_vec(), unread_now, rows));
        }
        self.cached.as_ref().map_or(&[], |(_, _, rows)| rows.as_slice())
    }

    /// Open or close the conversation whose oldest message is `key`.
    pub(crate) fn toggle(&mut self, key: MessageId) {
        if !self.open.remove(&key) {
            self.open.insert(key);
        }
        self.cached = None;
    }

    /// Open it, if it is not.
    pub(crate) fn open(&mut self, key: MessageId) {
        if self.open.insert(key) {
            self.cached = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::{Language, has};
    use ibercorreo_core::{Date, Message};

    #[test]
    fn every_string_is_translated() {
        for lang in Language::ALL {
            if lang == Language::En {
                continue;
            }
            for s in STRINGS {
                assert!(has(lang, s), "{lang:?}: {s}");
            }
        }
    }

    #[test]
    fn a_conversation_is_one_row_until_it_is_opened() {
        let today = Date::new(2026, 11, 15).unwrap();
        let mut session = Session::new(ibercorreo_engine::sample::store(today).unwrap(), today);
        let first = session.store.all_messages().find(|m| !m.subject.is_empty()).map(|m| (m.folder, m.subject.clone())).unwrap();
        let mut reply = Message::new(MessageId(0), first.0);
        reply.set_subject(&format!("Re: {}", first.1));
        session.store.add_message(reply).unwrap();
        let ids: Vec<MessageId> = session.store.messages_in(first.0).iter().map(|m| m.id).collect();
        let mut c = Conversations::default();
        let rows = c.rows(&session, &ids).to_vec();
        assert_eq!(rows.len(), ids.len() - 1, "two messages, one row");
        let Some(Kind::Head { count, key, open, .. }) = rows.iter().map(|r| r.kind).find(|k| matches!(k, Kind::Head { .. })) else {
            panic!("a conversation row")
        };
        assert_eq!((count, open), (2, false));
        c.toggle(key);
        assert_eq!(c.rows(&session, &ids).len(), ids.len(), "opened, every message has its row");
        assert!(c.rows(&session, &ids).iter().any(|r| r.kind == Kind::Child));
    }
}
