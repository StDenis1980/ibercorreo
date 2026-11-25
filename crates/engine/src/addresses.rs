//! Addresses (Tinta): the sender of a message added to the contacts in one step
//! (`contact.addSender`), and the addresses offered while a To, Cc or Bcc field is typed
//! (`contact.suggest`), as Outlook does. Suggestions come from the contacts first, then from the
//! people written to (the recipients of mail in a Sent folder), the most written to first. Case
//! and Latin accents are ignored, and a name matches from the start of any of its words.

use std::collections::HashMap;

use ibercorreo_core::{Address, Contact, Error, FolderRole, MessageId, Result};
use ibercorreo_store::search::fold;

use crate::Session;

/// The most suggestions a call returns.
pub const MAX_SUGGESTIONS: usize = 20;

/// One address offered for a recipient field.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Suggestion {
    pub name: Option<String>,
    pub email: String,
    /// The contact it comes from; `None` for someone only written to.
    pub contact: Option<u64>,
}

impl Suggestion {
    /// As it goes into a recipient field: `Name <address>`, or the bare address.
    pub fn field_text(&self) -> String {
        match &self.name {
            Some(n) if !n.trim().is_empty() => Address { name: Some(n.clone()), email: self.email.clone() }.to_string(),
            _ => self.email.clone(),
        }
    }
}

/// What a person typed matches `name` (from the start of any word) or `email` (from its start, or
/// from the start of a part of it after `.`, `_`, `-` or `@`).
fn matches(typed: &str, name: Option<&str>, email: &str) -> bool {
    if typed.is_empty() {
        return false;
    }
    let name = name.map(fold).unwrap_or_default();
    if name.starts_with(typed) || name.split_whitespace().any(|w| w.starts_with(typed)) {
        return true;
    }
    let email = email.to_lowercase();
    email.starts_with(typed) || email.split(['.', '_', '-', '@', '+']).any(|p| p.starts_with(typed))
}

/// Addresses for `typed` (the part of a recipient field after its last comma), at most `limit`.
pub fn suggest(s: &Session, typed: &str, limit: usize) -> Vec<Suggestion> {
    let typed = fold(typed.trim());
    let limit = limit.min(MAX_SUGGESTIONS);
    if typed.is_empty() || limit == 0 {
        return Vec::new();
    }
    let own: Vec<String> = s.store.accounts.iter().map(|a| a.address.email.to_lowercase()).collect();
    let mut seen: Vec<String> = Vec::new();
    let mut out = Vec::new();
    let mut contacts: Vec<&Contact> = s.store.all_contacts().iter().collect();
    contacts.sort_by_key(|c| fold(&c.name));
    for c in contacts {
        for a in &c.emails {
            let key = a.email.to_lowercase();
            if seen.contains(&key) || !matches(&typed, Some(&c.name), &a.email) {
                continue;
            }
            seen.push(key);
            let name = if c.name.trim().is_empty() { a.name.clone() } else { Some(c.name.clone()) };
            out.push(Suggestion { name, email: a.email.clone(), contact: Some(c.id) });
            if out.len() >= limit {
                return out;
            }
        }
    }
    // The people written to: how often, and the name used last.
    let sent: Vec<_> = s.store.folders().into_iter().filter(|f| f.role == FolderRole::Sent).map(|f| f.id).collect();
    let mut written: HashMap<String, (usize, Option<String>, String)> = HashMap::new();
    for m in s.store.all_messages().filter(|m| sent.contains(&m.folder)) {
        for a in m.to.iter().chain(&m.cc) {
            let key = a.email.to_lowercase();
            if seen.contains(&key) || own.contains(&key) || !matches(&typed, a.name.as_deref(), &a.email) {
                continue;
            }
            let e = written.entry(key).or_insert_with(|| (0, None, a.email.clone()));
            e.0 = e.0.saturating_add(1);
            if a.name.is_some() {
                e.1 = a.name.clone();
            }
        }
    }
    let mut written: Vec<_> = written.into_values().collect();
    written.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.2.to_lowercase().cmp(&b.2.to_lowercase())));
    out.extend(written.into_iter().take(limit.saturating_sub(out.len())).map(|(_, name, email)| Suggestion { name, email, contact: None }));
    out
}

/// The contact holding `email`, if any.
pub fn contact_with(s: &Session, email: &str) -> Option<u64> {
    let key = email.to_lowercase();
    s.store.all_contacts().iter().find(|c| c.emails.iter().any(|a| a.email.to_lowercase() == key)).map(|c| c.id)
}

/// The sender of message `id`, as a name for a new contact and the address.
pub fn sender(s: &Session, id: MessageId) -> Result<(String, Address)> {
    let m = s.store.message(id)?;
    let from = m.from.clone().ok_or_else(|| Error::invalid("message", "it has no sender"))?;
    let name = from.name.clone().filter(|n| !n.trim().is_empty()).unwrap_or_else(|| from.email.split('@').next().unwrap_or(&from.email).to_string());
    Ok((name, from))
}
