//! Address-book entries.

use serde::{Deserialize, Serialize};

use crate::Address;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Contact {
    pub id: u64,
    pub name: String,
    pub emails: Vec<Address>,
    pub phone: Option<String>,
    pub organization: Option<String>,
}

impl Contact {
    /// Whether `query` (already lowercased) matches the name, an address or the organization.
    pub fn matches(&self, query: &str) -> bool {
        query.is_empty()
            || self.name.to_lowercase().contains(query)
            || self.emails.iter().any(|a| a.email.to_lowercase().contains(query))
            || self.organization.as_deref().is_some_and(|o| o.to_lowercase().contains(query))
    }
}
