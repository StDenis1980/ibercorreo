//! Rules: what to do with mail as it arrives, as Outlook's client-side rules. A rule has a
//! condition, written as a search query ([`crate::search`]: `from:ana subject:invoice`,
//! `has:attachment`, `-is:read`…), and actions: mark as read, flag, move to a folder, or delete
//! (move to Deleted Items). Rules run in order; one with `stop` set ends the run for the messages
//! it matched.
//!
//! They run on mail new in an account's Inbox after Send/Receive (not on the first download of a
//! folder, nor on older mail fetched later), and on any folder when the person asks
//! ([`crate::Store::run_rules`]). A move into another account's folder, or into a folder that is
//! gone, is skipped and counted, never an error that stops the rest.

use ibercorreo_core::{AccountId, Error, FolderId, FolderRole, MessageId, Result, clamp_text};
use serde::{Deserialize, Serialize};

use crate::Store;
use crate::search::Query;

/// The most rules a store holds.
pub const MAX_RULES: usize = 200;
/// Longest rule name and query, in bytes.
pub const MAX_NAME: usize = 200;
pub const MAX_QUERY: usize = 1024;

/// What a rule does to a message it matches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "camelCase")]
pub enum RuleAction {
    MarkRead,
    Flag,
    /// Move to this folder (of the message's own account).
    Move {
        folder: FolderId,
    },
    /// Move to the account's Deleted Items (Trash).
    Delete,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rule {
    pub id: u32,
    pub name: String,
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Only mail of this account; `None`: every account.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<AccountId>,
    /// The condition, in search syntax; blank matches every message.
    pub query: String,
    pub actions: Vec<RuleAction>,
    /// Stop processing more rules for a message this one matched.
    #[serde(default)]
    pub stop: bool,
}

fn yes() -> bool {
    true
}

impl Rule {
    /// A rule with `name`, `query` and `actions`, enabled, for every account (its id is set when
    /// it is added).
    pub fn new(name: &str, query: &str, actions: Vec<RuleAction>) -> Rule {
        Rule { id: 0, name: name.into(), enabled: true, account: None, query: query.into(), actions, stop: false }
    }
}

/// What a run of the rules did.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct RulesReport {
    /// Messages at least one rule matched.
    pub matched: usize,
    pub moved: usize,
    pub deleted: usize,
    pub marked_read: usize,
    pub flagged: usize,
    /// Moves left undone: the folder is gone or belongs to another account.
    pub skipped: usize,
}

impl RulesReport {
    /// Whether anything changed.
    pub fn changed(&self) -> bool {
        self.moved > 0 || self.deleted > 0 || self.marked_read > 0 || self.flagged > 0
    }
}

impl Store {
    pub fn rules(&self) -> &[Rule] {
        &self.rules
    }

    pub fn rule(&self, id: u32) -> Result<&Rule> {
        self.rules.iter().find(|r| r.id == id).ok_or_else(|| Error::not_found("rule", id.to_string()))
    }

    /// Checked and trimmed: a name, at least one action, folders and account that exist, a move
    /// target in the rule's account (the account is taken from the folder when unset).
    fn clean_rule(&self, mut rule: Rule) -> Result<Rule> {
        rule.name = clamp_text(rule.name.trim(), MAX_NAME).to_string();
        rule.query = clamp_text(rule.query.trim(), MAX_QUERY).to_string();
        if rule.name.is_empty() {
            rule.name = if rule.query.is_empty() { "All messages".into() } else { rule.query.clone() };
        }
        let mut actions = Vec::new();
        for a in rule.actions {
            if !actions.contains(&a) {
                actions.push(a);
            }
        }
        rule.actions = actions;
        if rule.actions.is_empty() {
            return Err(Error::invalid("rule", "it needs at least one action"));
        }
        if rule.actions.iter().filter(|a| matches!(a, RuleAction::Move { .. } | RuleAction::Delete)).count() > 1 {
            return Err(Error::invalid("rule", "a message can be moved or deleted once"));
        }
        if let Some(a) = rule.account {
            self.account(a)?;
        }
        for a in &rule.actions {
            if let RuleAction::Move { folder } = a {
                let owner = self.folder(*folder)?.account;
                match rule.account {
                    None => rule.account = Some(owner),
                    Some(a) if a != owner => return Err(Error::invalid("rule", "the folder belongs to another account")),
                    Some(_) => {}
                }
            }
        }
        Ok(rule)
    }

    /// Add a rule at the end; its id is replaced and returned.
    pub fn add_rule(&mut self, rule: Rule) -> Result<u32> {
        if self.rules.len() >= MAX_RULES {
            return Err(Error::invalid("rule", format!("at most {MAX_RULES} rules")));
        }
        let mut rule = self.clean_rule(rule)?;
        let id = self.rules.iter().map(|r| r.id).max().map_or(1, |m| m.saturating_add(1));
        rule.id = id;
        self.rules.push(rule);
        Ok(id)
    }

    /// Replace rule `rule.id`, keeping its place.
    pub fn set_rule(&mut self, rule: Rule) -> Result<()> {
        let id = rule.id;
        let rule = self.clean_rule(rule)?;
        let slot = self.rules.iter_mut().find(|r| r.id == id).ok_or_else(|| Error::not_found("rule", id.to_string()))?;
        *slot = rule;
        Ok(())
    }

    pub fn remove_rule(&mut self, id: u32) -> Result<Rule> {
        let pos = self.rules.iter().position(|r| r.id == id).ok_or_else(|| Error::not_found("rule", id.to_string()))?;
        Ok(self.rules.remove(pos))
    }

    /// Move rule `id` one place earlier (`up`) or later in the order; at the end already, nothing
    /// changes.
    pub fn move_rule(&mut self, id: u32, up: bool) -> Result<()> {
        let pos = self.rules.iter().position(|r| r.id == id).ok_or_else(|| Error::not_found("rule", id.to_string()))?;
        let other = if up { pos.checked_sub(1) } else { pos.checked_add(1).filter(|p| *p < self.rules.len()) };
        if let Some(other) = other {
            self.rules.swap(pos, other);
        }
        Ok(())
    }

    /// Run the enabled rules (or only rule `only`, enabled or not) over `ids`, in the rules'
    /// order. Messages that are gone are passed over.
    pub fn run_rules(&mut self, ids: &[MessageId], only: Option<u32>) -> RulesReport {
        let rules: Vec<(Rule, Query)> = self
            .rules
            .iter()
            .filter(|r| match only {
                Some(id) => r.id == id,
                None => r.enabled,
            })
            .map(|r| (r.clone(), Query::parse(&r.query)))
            .collect();
        let mut report = RulesReport::default();
        if rules.is_empty() {
            return report;
        }
        for &id in ids {
            let mut matched = false;
            for (rule, query) in &rules {
                let Ok(m) = self.message(id) else { break };
                let Ok(account) = self.account_of(m.folder) else { break };
                // The body is read from disk only for a rule that looks at it.
                let content = if query.reads_body() { self.content(id).ok() } else { None };
                if rule.account.is_some_and(|a| a != account) || !query.matches(m, content.as_deref()) {
                    continue;
                }
                matched = true;
                self.apply_actions(id, account, &rule.actions, &mut report);
                if rule.stop {
                    break;
                }
            }
            if matched {
                report.matched = report.matched.saturating_add(1);
            }
        }
        report
    }

    fn apply_actions(&mut self, id: MessageId, account: AccountId, actions: &[RuleAction], report: &mut RulesReport) {
        // Marks first, so they travel with the message.
        for a in actions {
            let Ok(m) = self.message(id) else { return };
            match a {
                RuleAction::MarkRead if !m.seen => {
                    if let Ok(m) = self.message_mut(id) {
                        m.seen = true;
                        report.marked_read = report.marked_read.saturating_add(1);
                    }
                }
                RuleAction::Flag if !m.flagged => {
                    if let Ok(m) = self.message_mut(id) {
                        m.flagged = true;
                        report.flagged = report.flagged.saturating_add(1);
                    }
                }
                _ => {}
            }
        }
        for a in actions {
            let Ok(current) = self.message(id).map(|m| m.folder) else { return };
            let (target, deleting) = match a {
                RuleAction::Move { folder } => (Some(*folder), false),
                RuleAction::Delete => (self.folder_by_role(account, FolderRole::Trash).ok(), true),
                _ => continue,
            };
            let Some(target) = target.filter(|f| self.folder(*f).is_ok_and(|f| f.account == account)) else {
                report.skipped = report.skipped.saturating_add(1);
                continue;
            };
            if target == current {
                continue;
            }
            if self.move_message(id, target).is_ok() {
                if deleting {
                    report.deleted = report.deleted.saturating_add(1);
                } else {
                    report.moved = report.moved.saturating_add(1);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ibercorreo_core::{Account, Address, Message};

    fn account(email: &str) -> Account {
        Account {
            id: AccountId(0),
            name: email.into(),
            address: Address::parse(email).unwrap(),
            kind: ibercorreo_core::mail::AccountKind::Local,
            caldav: String::new(),
            signature: String::new(),
        }
    }

    fn mail(s: &mut Store, folder: FolderId, from: &str, subject: &str) -> MessageId {
        let mut m = Message::new(MessageId(0), folder);
        m.from = Some(Address::parse(from).unwrap());
        m.set_subject(subject);
        s.add_message(m).unwrap()
    }

    #[test]
    fn rules_move_mark_flag_delete_in_order_and_stop() {
        let mut s = Store::new();
        let a = s.first_account();
        let inbox = s.folder_by_role(a, FolderRole::Inbox).unwrap();
        let trash = s.folder_by_role(a, FolderRole::Trash).unwrap();
        let news = s.add_folder(a, "News", FolderRole::Other);
        let invoice = mail(&mut s, inbox, "Billing <billing@shop.es>", "Factura de octubre");
        let letter = mail(&mut s, inbox, "Newsletter <news@site.org>", "Novedades");
        let spam = mail(&mut s, inbox, "Promo <promo@spam.test>", "Oferta");
        let other = mail(&mut s, inbox, "Ana <ana@example.org>", "Hola");
        let flag = s.add_rule(Rule::new("Invoices", "subject:factura", vec![RuleAction::Flag, RuleAction::MarkRead])).unwrap();
        let mut stop = Rule::new("", "from:news", vec![RuleAction::Move { folder: news }]);
        stop.stop = true;
        s.add_rule(stop).unwrap();
        // Would delete the newsletter too, but the rule before it stops the run.
        s.add_rule(Rule::new("Junk", "from:promo OR from:news", vec![RuleAction::Delete])).unwrap();
        let report = s.run_rules(&[invoice, letter, spam, other, MessageId(999)], None);
        assert_eq!(report, RulesReport { matched: 3, moved: 1, deleted: 1, marked_read: 1, flagged: 1, skipped: 0 });
        let m = s.message(invoice).unwrap();
        assert!(m.flagged && m.seen && m.folder == inbox);
        assert_eq!(s.message(letter).unwrap().folder, news);
        assert_eq!(s.message(spam).unwrap().folder, trash);
        assert_eq!(s.message(other).unwrap().folder, inbox);
        // Disabled rules don't run, except when asked for by id.
        let mut r = s.rule(flag).unwrap().clone();
        r.enabled = false;
        s.set_rule(r).unwrap();
        let fresh = mail(&mut s, inbox, "Billing <billing@shop.es>", "Factura nueva");
        assert_eq!(s.run_rules(&[fresh], None).matched, 0);
        assert_eq!(s.run_rules(&[fresh], Some(flag)).flagged, 1);
        // The account of a move rule is the folder's.
        assert_eq!(s.rules().get(1).and_then(|r| r.account), Some(a));
        assert_eq!(s.rules().get(1).map(|r| r.name.as_str()), Some("from:news"));
    }

    #[test]
    fn rules_stay_in_their_account_and_are_checked() {
        let mut s = Store::new();
        let one = s.add_account(account("one@example.org")).unwrap();
        let two = s.add_account(account("two@example.org")).unwrap();
        let in1 = s.folder_by_role(one, FolderRole::Inbox).unwrap();
        let in2 = s.folder_by_role(two, FolderRole::Inbox).unwrap();
        let arch1 = s.folder_by_role(one, FolderRole::Archive).unwrap();
        let a = mail(&mut s, in1, "Ana <ana@example.org>", "x");
        let b = mail(&mut s, in2, "Ana <ana@example.org>", "x");
        s.add_rule(Rule::new("Ana", "from:ana", vec![RuleAction::Move { folder: arch1 }])).unwrap();
        let report = s.run_rules(&[a, b], None);
        assert_eq!((report.matched, report.moved), (1, 1));
        assert_eq!(s.message(b).unwrap().folder, in2, "a rule never moves mail between accounts");
        // Bad rules are refused.
        assert!(s.add_rule(Rule::new("none", "x", vec![])).is_err());
        assert!(s.add_rule(Rule::new("gone", "x", vec![RuleAction::Move { folder: FolderId(9999) }])).is_err());
        let mut cross = Rule::new("cross", "x", vec![RuleAction::Move { folder: arch1 }]);
        cross.account = Some(two);
        assert!(s.add_rule(cross).is_err());
        assert!(s.add_rule(Rule::new("twice", "x", vec![RuleAction::Delete, RuleAction::Move { folder: arch1 }])).is_err());
        assert!(s.remove_rule(77).is_err());
        // Removing an account takes its rules with it.
        s.remove_account(one).unwrap();
        assert!(s.rules().is_empty());
    }

    #[test]
    fn rules_reorder_save_and_survive_a_hand_edited_file() {
        let mut s = Store::new();
        let a = s.add_rule(Rule::new("a", "x", vec![RuleAction::Flag])).unwrap();
        let b = s.add_rule(Rule::new("b", "y", vec![RuleAction::MarkRead])).unwrap();
        s.move_rule(b, true).unwrap();
        s.move_rule(b, true).unwrap();
        assert_eq!(s.rules().iter().map(|r| r.id).collect::<Vec<_>>(), [b, a]);
        s.move_rule(a, false).unwrap();
        assert_eq!(s.rules().iter().map(|r| r.id).collect::<Vec<_>>(), [b, a]);
        let back = Store::from_json(&serde_json::to_vec(&s).unwrap()).unwrap();
        assert_eq!(back.rules(), s.rules());
        let mut v = serde_json::to_value(&s).unwrap();
        v["rules"] = serde_json::json!([
            {"id": 1, "name": "x", "query": "", "actions": [{"action": "flag"}]},
            {"id": 1, "name": "dup", "query": "", "actions": [{"action": "flag"}]},
            {"id": 2, "name": "gone", "query": "", "actions": [{"action": "move", "folder": 9999}]}
        ]);
        let edited = Store::from_json(&serde_json::to_vec(&v).unwrap()).unwrap();
        assert_eq!(edited.rules().len(), 2);
        assert!(edited.rules().iter().all(|r| r.enabled), "enabled by default");
        // A move to a folder that is gone is skipped, not an error.
        let mut edited = edited;
        let inbox = edited.folder_by_role(edited.first_account(), FolderRole::Inbox).unwrap();
        let m = mail(&mut edited, inbox, "Ana <ana@example.org>", "x");
        let report = edited.run_rules(&[m], None);
        assert_eq!((report.matched, report.flagged, report.skipped), (1, 1, 1));
    }
}
