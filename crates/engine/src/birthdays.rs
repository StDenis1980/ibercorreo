//! Birthdays in the calendar (Tinta), as Outlook's Birthdays calendar: every contact with a
//! birthday shows on that day each year, with the age they turn when the year is known. They are
//! not appointments: nothing is written to a calendar or sent to a server, and a changed birthday
//! shows at once. A 29 February birthday shows on 28 February in other years. The calendar list
//! shows or hides them (`calendar.birthdays`).

use ibercorreo_core::{Date, Error, Result};
use ibercorreo_store::Store;

/// The longest range [`between`] looks at, in days.
pub const MAX_DAYS: i64 = 366;
/// Years before this are taken as "year unknown" (Apple writes 1604 for a birthday without one).
const FIRST_YEAR: i32 = 1900;

/// A contact's birthday on a day of the calendar.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Birthday {
    pub contact: u64,
    pub name: String,
    /// The day it falls on in that year.
    pub date: Date,
    /// The age they turn that day, when the year they were born is known.
    pub age: Option<i32>,
    /// Their first address, to write to them.
    pub email: Option<String>,
}

/// `born`'s birthday in `year`: the same day, or 28 February for 29 February in other years.
fn in_year(born: Date, year: i32) -> Option<Date> {
    Date::new(year, born.month(), born.day()).or_else(|_| Date::new(year, born.month(), born.day().saturating_sub(1))).ok()
}

/// The birthdays from `from` to `to` (both included), by day and then name.
pub fn between(store: &Store, from: Date, to: Date) -> Result<Vec<Birthday>> {
    if to < from || to.day_number() - from.day_number() >= MAX_DAYS {
        return Err(Error::invalid("to", format!("a range of one day to {MAX_DAYS}")));
    }
    let mut out = Vec::new();
    for c in store.all_contacts() {
        let Some(born) = c.birthday else { continue };
        for year in from.year()..=to.year() {
            let Some(date) = in_year(born, year).filter(|d| *d >= from && *d <= to && *d >= born) else { continue };
            let age = (born.year() >= FIRST_YEAR).then(|| year - born.year());
            let name = if c.name.trim().is_empty() { c.emails.first().map(|a| a.email.clone()).unwrap_or_default() } else { c.name.clone() };
            out.push(Birthday { contact: c.id, name, date, age, email: c.emails.first().map(|a| a.to_string()) });
        }
    }
    out.sort_by(|a, b| a.date.cmp(&b.date).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ibercorreo_core::{Address, Contact};

    fn d(y: i32, m: u8, day: u8) -> Date {
        Date::new(y, m, day).unwrap()
    }

    #[test]
    fn birthdays_fall_each_year_with_the_age_turned() {
        let mut s = Store::new();
        for (name, born) in [("Olena", Some(d(1989, 3, 9))), ("Leap", Some(d(2000, 2, 29))), ("Unknown year", Some(d(1604, 12, 24))), ("None", None)]
        {
            let mut c = Contact::new(name);
            c.birthday = born;
            c.emails = vec![Address::new(Some(name), "x@example.org").unwrap()];
            s.add_contact(c);
        }
        let year = between(&s, d(2026, 1, 1), d(2026, 12, 31)).unwrap();
        let got: Vec<_> = year.iter().map(|b| (b.name.as_str(), b.date, b.age)).collect();
        assert_eq!(got, [("Leap", d(2026, 2, 28), Some(26)), ("Olena", d(2026, 3, 9), Some(37)), ("Unknown year", d(2026, 12, 24), None)]);
        assert_eq!(between(&s, d(2028, 2, 29), d(2028, 2, 29)).unwrap()[0].date, d(2028, 2, 29), "on the day in a leap year");
        // Across the new year, and never before they were born.
        let turn = between(&s, d(2026, 12, 20), d(2027, 3, 10)).unwrap();
        assert_eq!(turn.iter().map(|b| b.date).collect::<Vec<_>>(), [d(2026, 12, 24), d(2027, 2, 28), d(2027, 3, 9)]);
        assert!(between(&s, d(1980, 1, 1), d(1980, 12, 31)).unwrap().iter().all(|b| b.name == "Unknown year"));
        assert!(between(&s, d(2026, 1, 2), d(2026, 1, 1)).is_err());
        assert!(between(&s, d(2026, 1, 1), d(2027, 1, 2)).is_err(), "at most 366 days");
    }
}
