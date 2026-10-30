//! Tinta Office: whether automatic error reports may be sent, kept with the person's other
//! choices in the store. The engine changes it (`file.errorReports`); the desktop app asks once,
//! writes and sends the reports, and makes the random installation id.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The saved choice.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ErrorReports {
    /// `None` until the person answered the first-start question.
    pub consent: Option<bool>,
    /// The random id reports are filed under on the server (made by the desktop app when reports
    /// are first allowed). Not tied to the person.
    pub install_id: String,
    /// An id whose reports the server should delete; the desktop app clears it once the server
    /// confirmed, so a deletion asked for offline happens at the next start.
    pub forget_pending: Option<String>,
}

impl ErrorReports {
    pub fn is_default(&self) -> bool {
        *self == ErrorReports::default()
    }
}

/// A valid installation id: the lowercase 8-4-4-4-12 hex form.
pub fn id_ok(id: &str) -> bool {
    let groups: Vec<&str> = id.split('-').collect();
    groups.len() == 5
        && groups.iter().zip([8, 4, 4, 4, 12]).all(|(g, n)| g.len() == n && g.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
}

/// A damaged saved value reads as "not asked yet" and never stops the mailbox from loading;
/// malformed ids are dropped.
pub fn lenient<'de, D: serde::Deserializer<'de>>(d: D) -> Result<ErrorReports, D::Error> {
    let value = Value::deserialize(d)?;
    let mut r: ErrorReports = serde_json::from_value(value).unwrap_or_default();
    if !id_ok(&r.install_id) {
        r.install_id.clear();
    }
    r.forget_pending = r.forget_pending.filter(|id| id_ok(id));
    Ok(r)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn damaged_saved_values_read_as_not_asked() {
        #[derive(Deserialize)]
        struct W {
            #[serde(deserialize_with = "lenient")]
            r: ErrorReports,
        }
        let w: W = serde_json::from_value(json!({"r": {"consent": "maybe"}})).unwrap();
        assert_eq!(w.r, ErrorReports::default());
        let w: W = serde_json::from_value(json!({"r": {"consent": true, "install_id": "../../etc", "forget_pending": "x"}})).unwrap();
        assert_eq!(w.r, ErrorReports { consent: Some(true), ..Default::default() });
        assert!(id_ok("0f8fad5b-d9cb-469f-a165-70867728950e"));
        assert!(!id_ok("0F8FAD5B-d9cb-469f-a165-70867728950e"));
    }
}
