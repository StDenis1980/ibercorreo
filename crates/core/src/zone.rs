//! Time zones: converting wall-clock times between a named zone and UTC, with the IANA database
//! (through `jiff`, bundled into the program, so Windows, Linux, macOS and the browser agree).
//!
//! Calendar files name zones three ways: IANA ids (`Europe/Lisbon`), Windows names, as Outlook
//! and Exchange write them (`GMT Standard Time`), and ids with a vendor prefix
//! (`/mozilla.org/20050126_1/Europe/Lisbon`). [`Zone::named`] understands all three.

use jiff::tz::TimeZone;

use crate::{DateTime, Error, Result};

/// A named time zone.
#[derive(Debug, Clone)]
pub struct Zone {
    name: String,
    tz: TimeZone,
}

impl PartialEq for Zone {
    fn eq(&self, other: &Zone) -> bool {
        self.name == other.name
    }
}

impl Eq for Zone {}

/// Longest zone id looked up; real ones are under 40 bytes.
const MAX_ID: usize = 256;

impl Zone {
    pub fn utc() -> Zone {
        Zone { name: "UTC".into(), tz: TimeZone::UTC }
    }

    /// The zone `id` names, or `None` when the database does not know it.
    pub fn named(id: &str) -> Option<Zone> {
        let id = id.trim().trim_matches('"');
        if id.is_empty() || id.len() > MAX_ID {
            return None;
        }
        if id.eq_ignore_ascii_case("utc") || id.eq_ignore_ascii_case("gmt") || id == "Z" || id.eq_ignore_ascii_case("etc/utc") {
            return Some(Zone::utc());
        }
        let iana = WINDOWS.iter().find(|(w, _)| w.eq_ignore_ascii_case(id)).map_or(id, |(_, iana)| *iana);
        if let Ok(tz) = TimeZone::get(iana) {
            return Some(Zone { name: iana.to_string(), tz });
        }
        // A vendor prefix: try the last two path segments, then the last one.
        let parts: Vec<&str> = id.split('/').filter(|p| !p.is_empty()).collect();
        for take in [3, 2] {
            let Some(tail) = parts.len().checked_sub(take).and_then(|start| parts.get(start..)) else { continue };
            let candidate = tail.join("/");
            if candidate != id
                && let Ok(tz) = TimeZone::get(&candidate)
            {
                return Some(Zone { name: candidate, tz });
            }
        }
        None
    }

    /// The zone the computer is set to, or UTC when it cannot be told (as in a browser).
    pub fn system() -> Zone {
        match TimeZone::try_system() {
            Ok(tz) => match tz.iana_name() {
                Some(name) if name != "UTC" && name != "Etc/UTC" => Zone { name: name.to_string(), tz },
                _ => Zone::utc(),
            },
            Err(_) => Zone::utc(),
        }
    }

    /// The IANA id, or `UTC`.
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn is_utc(&self) -> bool {
        self.name == "UTC"
    }

    /// The wall-clock time here at UTC time `t`. Out-of-range input comes back unchanged.
    pub fn from_utc(&self, t: DateTime) -> DateTime {
        self.try_from_utc(t).unwrap_or(t)
    }

    /// The UTC time of wall-clock time `t` here. A time skipped by a clock change counts as the
    /// time after it, and a time that happens twice as the first, as RFC 5545 §3.3.5 says.
    pub fn to_utc(&self, t: DateTime) -> DateTime {
        self.try_to_utc(t).unwrap_or(t)
    }

    /// The offset from UTC at UTC time `t`, in minutes (east positive).
    pub fn offset_minutes(&self, t: DateTime) -> i32 {
        jiff::Timestamp::from_second(t.to_unix()).map_or(0, |ts| self.tz.to_offset(ts).seconds() / 60)
    }

    /// `UTC+01:00`, `UTC−03:30`, `UTC` at UTC time `t`.
    pub fn offset_label(&self, t: DateTime) -> String {
        let m = self.offset_minutes(t);
        if m == 0 {
            return "UTC".into();
        }
        let sign = if m < 0 { '−' } else { '+' };
        format!("UTC{sign}{:02}:{:02}", m.unsigned_abs() / 60, m.unsigned_abs() % 60)
    }

    fn try_from_utc(&self, t: DateTime) -> Result<DateTime> {
        let ts = jiff::Timestamp::from_second(t.to_unix()).map_err(bad)?;
        from_civil(self.tz.to_datetime(ts))
    }

    fn try_to_utc(&self, t: DateTime) -> Result<DateTime> {
        let civil = to_civil(t)?;
        let zoned = self.tz.to_ambiguous_zoned(civil).compatible().map_err(bad)?;
        DateTime::from_unix(zoned.timestamp().as_second())
    }
}

/// Every zone id the database has, sorted, for a settings list.
pub fn all_names() -> Vec<String> {
    let mut v: Vec<String> = jiff::tz::db().available().map(|n| n.to_string()).filter(|n| n.contains('/') && !n.starts_with("Etc/")).collect();
    v.sort();
    v.dedup();
    v
}

fn bad(e: jiff::Error) -> Error {
    Error::invalid("time zone", e.to_string())
}

fn to_civil(t: DateTime) -> Result<jiff::civil::DateTime> {
    let n = |v: u8| i8::try_from(v).map_err(|_| Error::invalid("time", "out of range"));
    let year = i16::try_from(t.date.year()).map_err(|_| Error::invalid("time", "year out of range"))?;
    // jiff has no leap second; :60 reads as :59.
    jiff::civil::DateTime::new(year, n(t.date.month())?, n(t.date.day())?, n(t.hour())?, n(t.minute())?, n(t.second().min(59))?, 0).map_err(bad)
}

fn from_civil(c: jiff::civil::DateTime) -> Result<DateTime> {
    let n = |v: i8| u8::try_from(v).map_err(|_| Error::invalid("time", "out of range"));
    let date = crate::Date::new(i32::from(c.year()), n(c.month())?, n(c.day())?)?;
    DateTime::new(date, n(c.hour())?, n(c.minute())?, n(c.second())?)
}

/// Windows zone names to IANA ids, from the Unicode CLDR `windowsZones` table (the "001"
/// territory of each), for the zones calendar invitations most often carry.
const WINDOWS: &[(&str, &str)] = &[
    ("GMT Standard Time", "Europe/London"),
    ("Greenwich Standard Time", "Atlantic/Reykjavik"),
    ("W. Europe Standard Time", "Europe/Berlin"),
    ("Central Europe Standard Time", "Europe/Budapest"),
    ("Romance Standard Time", "Europe/Paris"),
    ("Central European Standard Time", "Europe/Warsaw"),
    ("W. Central Africa Standard Time", "Africa/Lagos"),
    ("E. Europe Standard Time", "Europe/Chisinau"),
    ("FLE Standard Time", "Europe/Kyiv"),
    ("GTB Standard Time", "Europe/Bucharest"),
    ("Kaliningrad Standard Time", "Europe/Kaliningrad"),
    ("Belarus Standard Time", "Europe/Minsk"),
    ("Russian Standard Time", "Europe/Moscow"),
    ("Turkey Standard Time", "Europe/Istanbul"),
    ("Israel Standard Time", "Asia/Jerusalem"),
    ("Egypt Standard Time", "Africa/Cairo"),
    ("South Africa Standard Time", "Africa/Johannesburg"),
    ("Morocco Standard Time", "Africa/Casablanca"),
    ("Azores Standard Time", "Atlantic/Azores"),
    ("Cape Verde Standard Time", "Atlantic/Cape_Verde"),
    ("Arabian Standard Time", "Asia/Dubai"),
    ("Arab Standard Time", "Asia/Riyadh"),
    ("Iran Standard Time", "Asia/Tehran"),
    ("Pakistan Standard Time", "Asia/Karachi"),
    ("India Standard Time", "Asia/Calcutta"),
    ("SE Asia Standard Time", "Asia/Bangkok"),
    ("China Standard Time", "Asia/Shanghai"),
    ("Singapore Standard Time", "Asia/Singapore"),
    ("Taipei Standard Time", "Asia/Taipei"),
    ("Tokyo Standard Time", "Asia/Tokyo"),
    ("Korea Standard Time", "Asia/Seoul"),
    ("AUS Eastern Standard Time", "Australia/Sydney"),
    ("New Zealand Standard Time", "Pacific/Auckland"),
    ("Hawaiian Standard Time", "Pacific/Honolulu"),
    ("Alaskan Standard Time", "America/Anchorage"),
    ("Pacific Standard Time", "America/Los_Angeles"),
    ("US Mountain Standard Time", "America/Phoenix"),
    ("Mountain Standard Time", "America/Denver"),
    ("Central Standard Time", "America/Chicago"),
    ("Central Standard Time (Mexico)", "America/Mexico_City"),
    ("Eastern Standard Time", "America/New_York"),
    ("SA Pacific Standard Time", "America/Bogota"),
    ("Venezuela Standard Time", "America/Caracas"),
    ("Atlantic Standard Time", "America/Halifax"),
    ("Pacific SA Standard Time", "America/Santiago"),
    ("E. South America Standard Time", "America/Sao_Paulo"),
    ("Argentina Standard Time", "America/Buenos_Aires"),
    ("UTC", "UTC"),
];

#[cfg(test)]
mod tests;
