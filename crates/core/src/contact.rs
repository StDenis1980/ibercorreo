//! Address-book entries.

use serde::{Deserialize, Serialize};

use crate::{Address, CalendarRemote, Date, EventRemote};

/// A telephone number and what it is (`mobile`, `work`, `home`, `fax`, or empty for other).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Phone {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub kind: String,
    pub number: String,
}

/// An address book: a local one, or one mirrored from a CardDAV server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AddressBook {
    pub id: u32,
    pub name: String,
    /// The server collection (its path and change marker) this book mirrors.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<CalendarRemote>,
}

/// The kinds a phone can be, as the interface offers them.
pub const PHONE_KINDS: &[&str] = &["mobile", "work", "home", "fax", ""];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Contact {
    pub id: u64,
    /// The address book it is in.
    #[serde(default)]
    pub book: u32,
    /// The vCard `UID`, stable across files and servers; empty for a contact that never had one.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub uid: String,
    pub name: String,
    pub emails: Vec<Address>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub phones: Vec<Phone>,
    pub organization: Option<String>,
    /// Job title (`TITLE`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub birthday: Option<Date>,
    /// vCard lines Tinta Correo does not read (postal addresses, photos, web sites, `X-…`), kept
    /// as they came so the card is written back whole.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub kept: Vec<String>,
    /// The CardDAV resource it was synced from (path, version, changed here); `raw` is unused,
    /// since [`Contact::kept`] already holds what is not read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<EventRemote>,
    /// Store format 2 and older held one phone here; [`Contact::upgrade`] moves it to `phones`.
    #[serde(default, rename = "phone", skip_serializing)]
    pub legacy_phone: Option<String>,
}

impl Contact {
    /// A contact with only a name.
    pub fn new(name: &str) -> Contact {
        Contact {
            id: 0,
            book: 0,
            uid: String::new(),
            name: name.to_string(),
            emails: Vec::new(),
            phones: Vec::new(),
            organization: None,
            title: None,
            note: String::new(),
            birthday: None,
            kept: Vec::new(),
            remote: None,
            legacy_phone: None,
        }
    }

    /// Bring a contact read from an older store up to date.
    pub fn upgrade(&mut self) {
        if let Some(number) = self.legacy_phone.take().filter(|n| !n.trim().is_empty()) {
            self.phones.insert(0, Phone { kind: String::new(), number });
        }
    }

    /// Whether `query` (already lowercased) matches the name, an address, a phone or the
    /// organization.
    pub fn matches(&self, query: &str) -> bool {
        query.is_empty()
            || self.name.to_lowercase().contains(query)
            || self.emails.iter().any(|a| a.email.to_lowercase().contains(query))
            || self.phones.iter().any(|p| p.number.contains(query))
            || self.organization.as_deref().is_some_and(|o| o.to_lowercase().contains(query))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_contacts_keep_their_phone() {
        let mut c: Contact = serde_json::from_str(r#"{"id":3,"name":"Ana","emails":[],"phone":"+351 210 000 000","organization":null}"#).unwrap();
        c.upgrade();
        assert_eq!(c.phones, [Phone { kind: String::new(), number: "+351 210 000 000".into() }]);
        assert!(!serde_json::to_string(&c).unwrap().contains("\"phone\""), "written back in the new form only");
        assert!(c.matches("210 000"));
    }
}
