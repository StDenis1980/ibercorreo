//! Tinta: conversations, as mail programs group them: messages whose subjects are the same once
//! `Re:`, `Fwd:` and the like are taken off ([`ibercorreo_core::mail::conversation_topic`]).
//! Mail without a subject is never grouped.

use std::collections::HashMap;

use ibercorreo_core::mail::conversation_topic;
use ibercorreo_core::{Message, MessageId};

/// `messages`, newest first, as conversations: each conversation newest first, in the order of
/// its newest message. A message alone is a conversation of one.
pub fn group(messages: &[&Message]) -> Vec<Vec<MessageId>> {
    let mut groups: Vec<Vec<MessageId>> = Vec::new();
    let mut by_topic: HashMap<String, usize> = HashMap::new();
    for m in messages {
        match conversation_topic(&m.subject) {
            Some(topic) => match by_topic.get(&topic) {
                Some(&i) => {
                    if let Some(g) = groups.get_mut(i) {
                        g.push(m.id);
                    }
                }
                None => {
                    by_topic.insert(topic, groups.len());
                    groups.push(vec![m.id]);
                }
            },
            None => groups.push(vec![m.id]),
        }
    }
    groups
}

#[cfg(test)]
mod tests {
    use super::*;
    use ibercorreo_core::FolderId;

    #[test]
    fn replies_join_their_conversation_in_its_newest_place() {
        let msg = |id: u64, subject: &str| {
            let mut m = Message::new(MessageId(id), FolderId(0));
            m.set_subject(subject);
            m
        };
        let all = [msg(5, "Re: Budget"), msg(4, "Lunch?"), msg(3, ""), msg(2, "Budget"), msg(1, "")];
        let refs: Vec<&Message> = all.iter().collect();
        let ids = |g: &Vec<MessageId>| g.iter().map(|i| i.0).collect::<Vec<_>>();
        let groups = group(&refs);
        assert_eq!(groups.iter().map(ids).collect::<Vec<_>>(), [vec![5, 2], vec![4], vec![3], vec![1]], "no subject, no grouping");
    }
}
