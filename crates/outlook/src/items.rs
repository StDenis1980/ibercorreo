//! Calendar items and contacts (MS-OXOCAL, MS-OXOCNTC) read into the model. Most of what makes
//! them what they are (an appointment's times and place, a contact's e-mail addresses) is in
//! named properties, found through the file's [`NameMap`].

use ibercorreo_core::calendar::{When, clean_reminders};
use ibercorreo_core::{Address, Contact, Date, DateTime, Event, EventId, Phone, Zone, clamp_text};

use crate::named::{NameMap, PSETID_ADDRESS, PSETID_APPOINTMENT, PSETID_COMMON, PSETID_MEETING};
use crate::ndb::u16_at;
use crate::props::{Bag, PT_BINARY, PT_BOOLEAN, PT_LONG, PT_TIME, Raw, from_filetime, tag, text_of, utf16};
use crate::recur;

/// What kind of item a message class names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Appointment,
    Contact,
    Other,
}

pub(crate) fn kind(raw: &Raw) -> Kind {
    let class = text_of(&raw.props, tag::MESSAGE_CLASS).unwrap_or_default().to_ascii_lowercase();
    if class.starts_with("ipm.appointment") {
        Kind::Appointment
    } else if class.starts_with("ipm.contact") {
        Kind::Contact
    } else {
        Kind::Other
    }
}

// Appointment (PSETID_Appointment) and reminder (PSETID_Common) names.
const LOCATION: u32 = 0x8208;
const START_WHOLE: u32 = 0x820d;
const END_WHOLE: u32 = 0x820e;
const ALL_DAY: u32 = 0x8215;
const RECUR: u32 = 0x8216;
const RECURRING: u32 = 0x8223;
const EXCEPTION_REPLACE_TIME: u32 = 0x8228;
const TIME_ZONE_START_DISPLAY: u32 = 0x825e;
const REMINDER_DELTA: u32 = 0x8501;
const REMINDER_SET: u32 = 0x8503;
// Meeting (PSETID_Meeting): the item's id across mailboxes.
const GLOBAL_OBJECT_ID: u32 = 0x0003;
const CLEAN_GLOBAL_OBJECT_ID: u32 = 0x0023;
/// `PidTagStartDate` and `PidTagEndDate`, which most appointments also carry.
const START_DATE: u16 = 0x0060;
const END_DATE: u16 = 0x0061;

/// A raw item's named properties.
struct Named<'a> {
    bag: &'a Bag,
    names: &'a NameMap,
}

impl Named<'_> {
    fn fixed(&self, set: &crate::named::Guid, id: u32, kinds: &[u16]) -> Option<u64> {
        self.bag.fixed(self.names.tag(set, id)?, kinds)
    }
    fn time(&self, set: &crate::named::Guid, id: u32) -> Option<DateTime> {
        self.fixed(set, id, &[PT_TIME]).and_then(from_filetime)
    }
    fn flag(&self, set: &crate::named::Guid, id: u32) -> bool {
        self.fixed(set, id, &[PT_BOOLEAN]).is_some_and(|v| v != 0)
    }
    fn bytes(&self, set: &crate::named::Guid, id: u32) -> Option<&[u8]> {
        self.bag.bytes(self.names.tag(set, id)?, PT_BINARY)
    }
    fn text(&self, set: &crate::named::Guid, id: u32) -> Option<String> {
        text_of(self.bag, self.names.tag(set, id)?).map(|t| t.trim().to_string()).filter(|t| !t.is_empty())
    }
}

fn body(bag: &Bag) -> String {
    let text = text_of(bag, tag::BODY).unwrap_or_default().replace("\r\n", "\n");
    clamp_text(text.trim(), 64 * 1024).to_string()
}

/// The Windows zone id an appointment was made in (`TimeZoneDefinition`'s key name).
fn zone_key(b: &[u8]) -> Option<String> {
    let chars = usize::from(u16_at(b, 6)?);
    let name = utf16(b.get(8..8 + chars.checked_mul(2)?)?);
    Some(name).filter(|n| !n.trim().is_empty())
}

/// The date of an all-day boundary saved as a UTC time: midnight where the event was made, the
/// nearest midnight UTC wherever that is less than 12 hours from UTC (every zone but a few
/// Pacific islands).
fn nearest_day(t: DateTime) -> Date {
    DateTime::from_unix(t.to_unix().saturating_add(12 * 3600)).map_or(t.date, |n| n.date)
}

/// An appointment and the changed occurrences of its series, as events (calendar 0). `None` for
/// an item without a start.
pub(crate) fn appointment(raw: &Raw, names: &NameMap) -> Option<Vec<Event>> {
    let n = Named { bag: &raw.props, names };
    let key = n.bytes(&PSETID_APPOINTMENT, TIME_ZONE_START_DISPLAY).and_then(zone_key);
    let zone = key.as_deref().and_then(Zone::named);
    let master = event(&raw.props, names, zone.as_ref(), key.as_deref(), None)?;
    let mut out = vec![master];
    let series = (n.flag(&PSETID_APPOINTMENT, RECURRING) || n.bytes(&PSETID_APPOINTMENT, RECUR).is_some())
        .then(|| n.bytes(&PSETID_APPOINTMENT, RECUR).and_then(recur::read))
        .flatten();
    if let (Some(s), Some(m)) = (series, out.first_mut()) {
        m.rrule = Some(s.rrule);
        m.exdates = s.deleted;
        // Each changed occurrence is an attached message with its own details.
        for a in &raw.attachments {
            let Some(e) = a.embedded.as_deref() else { continue };
            let en = Named { bag: &e.props, names };
            let Some(replaces) = en.time(&PSETID_APPOINTMENT, EXCEPTION_REPLACE_TIME) else { continue };
            let day = zone.as_ref().map_or(replaces.date, |z| z.from_utc(replaces).date);
            if !s.changed.contains(&day) {
                continue;
            }
            let Some(master) = out.first() else { break };
            let base = master.clone();
            if let Some(mut x) = event(&e.props, names, zone.as_ref(), key.as_deref(), Some(&base)) {
                x.uid = base.uid.clone();
                x.rrule = None;
                x.exdates = Vec::new();
                x.recurrence_id = Some(day);
                out.push(x);
            }
        }
    }
    Some(out)
}

/// One event from an appointment's properties; `base` fills what an exception leaves out.
fn event(bag: &Bag, names: &NameMap, zone: Option<&Zone>, key: Option<&str>, base: Option<&Event>) -> Option<Event> {
    let n = Named { bag, names };
    let start = n.time(&PSETID_APPOINTMENT, START_WHOLE).or_else(|| bag.fixed(START_DATE, &[PT_TIME]).and_then(from_filetime))?;
    let end = n
        .time(&PSETID_APPOINTMENT, END_WHOLE)
        .or_else(|| bag.fixed(END_DATE, &[PT_TIME]).and_then(from_filetime))
        .filter(|e| e.to_unix() >= start.to_unix())
        .unwrap_or(start);
    let all_day = match n.fixed(&PSETID_APPOINTMENT, ALL_DAY, &[PT_BOOLEAN]) {
        Some(v) => v != 0,
        None => base.is_some_and(|b| matches!(b.when, When::AllDay { .. })),
    };
    let (when, tzid) = if all_day {
        let first = nearest_day(start);
        let last = nearest_day(end).max(first.add_days(1).ok()?);
        (When::AllDay { start: first, end: last }, None)
    } else {
        match zone {
            Some(z) => (When::Timed { start: z.from_utc(start), end: z.from_utc(end), utc: false }, key.map(str::to_string)),
            None => (When::Timed { start, end, utc: true }, None),
        }
    };
    let summary = text_of(bag, tag::SUBJECT).map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let location = n.text(&PSETID_APPOINTMENT, LOCATION);
    let description = body(bag);
    let reminders = match n.fixed(&PSETID_COMMON, REMINDER_SET, &[PT_BOOLEAN]) {
        Some(0) => Vec::new(),
        Some(_) => n.fixed(&PSETID_COMMON, REMINDER_DELTA, &[PT_LONG]).map(|m| clean_reminders(vec![i64::from(m as u32 as i32)])).unwrap_or_default(),
        None => base.map(|b| b.reminders.clone()).unwrap_or_default(),
    };
    let uid = n
        .bytes(&PSETID_MEETING, CLEAN_GLOBAL_OBJECT_ID)
        .or_else(|| n.bytes(&PSETID_MEETING, GLOBAL_OBJECT_ID))
        .filter(|b| !b.is_empty() && b.len() <= 512)
        .map(|b| b.iter().map(|x| format!("{x:02X}")).collect::<String>())
        .unwrap_or_default();
    Some(Event {
        id: EventId(0),
        calendar: 0,
        uid,
        summary: summary.or_else(|| base.map(|b| b.summary.clone())).unwrap_or_default(),
        location: location.or_else(|| base.map(|b| b.location.clone())).unwrap_or_default(),
        description: if description.is_empty() { base.map(|b| b.description.clone()).unwrap_or_default() } else { description },
        when,
        tzid,
        rrule: None,
        exdates: Vec::new(),
        recurrence_id: None,
        reminders,
        remote: None,
    })
}

// Contact names (PSETID_Address): each of the three e-mail addresses has a display name and
// the address.
const EMAILS: [(u32, u32); 3] = [(0x8080, 0x8083), (0x8090, 0x8093), (0x80a0, 0x80a3)];
const WEB_PAGE: u32 = 0x802b;

const GIVEN_NAME: u16 = 0x3a06;
const SURNAME: u16 = 0x3a11;
const COMPANY: u16 = 0x3a16;
const TITLE: u16 = 0x3a17;
const BIRTHDAY: u16 = 0x3a42;
const PHONES: [(u16, &str); 9] = [
    (0x3a1c, "mobile"),
    (0x3a08, "work"),
    (0x3a1b, "work"),
    (0x3a09, "home"),
    (0x3a2f, "home"),
    (0x3a24, "fax"),
    (0x3a25, "fax"),
    (0x3a1a, ""),
    (0x3a1f, ""),
];
/// Business and home postal addresses: street, city, state, postal code, country.
const ADDRESSES: [(&str, [u16; 5]); 2] = [("WORK", [0x3a29, 0x3a27, 0x3a28, 0x3a2a, 0x3a26]), ("HOME", [0x3a5d, 0x3a59, 0x3a5c, 0x3a5b, 0x3a5a])];

fn vcard_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace(';', "\\;").replace(',', "\\,").replace("\r\n", "\\n").replace(['\n', '\r'], "\\n")
}

/// A contact (in book 0). `None` for one with neither a name nor an address.
pub(crate) fn contact(raw: &Raw, names: &NameMap) -> Option<Contact> {
    let bag = &raw.props;
    let n = Named { bag, names };
    let text = |id: u16| text_of(bag, id).map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
    let mut emails = Vec::new();
    for (name_id, addr_id) in EMAILS {
        let Some(addr) = n.text(&PSETID_ADDRESS, addr_id) else { continue };
        let display = n.text(&PSETID_ADDRESS, name_id);
        // Outlook's display names read "Name (address)"; the address alone says as much.
        let display = display.filter(|d| !d.contains(&addr));
        // An Exchange address (`/O=…/CN=…`) is not one mail can be sent to: `Address` refuses it.
        if let Ok(a) = Address::new(display.as_deref(), &addr)
            && !emails.iter().any(|e: &Address| e.email.eq_ignore_ascii_case(&a.email))
        {
            emails.push(a);
        }
    }
    let name = text(tag::DISPLAY_NAME)
        .or_else(|| {
            let parts: Vec<String> = [text(GIVEN_NAME), text(SURNAME)].into_iter().flatten().collect();
            Some(parts.join(" ")).filter(|p| !p.is_empty())
        })
        .or_else(|| text(COMPANY))
        .or_else(|| emails.first().map(|a| a.email.clone()))?;
    let mut c = Contact::new(clamp_text(&name, 512));
    c.emails = emails;
    for (id, kind) in PHONES {
        if let Some(number) = text(id)
            && !c.phones.iter().any(|p| p.number == number)
        {
            c.phones.push(Phone { kind: kind.to_string(), number: clamp_text(&number, 256).to_string() });
        }
    }
    c.organization = text(COMPANY);
    c.title = text(TITLE);
    c.note = body(bag);
    c.birthday = bag.fixed(BIRTHDAY, &[PT_TIME]).and_then(from_filetime).map(nearest_day);
    for (kind, ids) in ADDRESSES {
        let parts: Vec<String> = ids.iter().map(|&id| text(id).map(|t| vcard_escape(&t)).unwrap_or_default()).collect();
        if parts.iter().any(|p| !p.is_empty()) {
            c.kept.push(format!("ADR;TYPE={kind}:;;{}", parts.join(";")));
        }
    }
    if let Some(url) = n.text(&PSETID_ADDRESS, WEB_PAGE).or_else(|| text(0x3a51)) {
        c.kept.push(format!("URL:{}", vcard_escape(&url)));
    }
    Some(c)
}

// Tested through the archive reader: crate::pst::tests::items.
