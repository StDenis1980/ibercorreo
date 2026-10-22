use super::*;
use crate::Date;

fn at(y: i32, m: u8, d: u8, h: u8, mi: u8) -> DateTime {
    DateTime::new(Date::new(y, m, d).unwrap(), h, mi, 0).unwrap()
}

#[test]
fn converts_both_ways_across_summer_time() {
    let lisbon = Zone::named("Europe/Lisbon").unwrap();
    // Summer (WEST, +1) and winter (WET, +0).
    assert_eq!(lisbon.from_utc(at(2026, 7, 1, 8, 0)), at(2026, 7, 1, 9, 0));
    assert_eq!(lisbon.from_utc(at(2026, 12, 1, 8, 0)), at(2026, 12, 1, 8, 0));
    assert_eq!(lisbon.to_utc(at(2026, 7, 1, 9, 0)), at(2026, 7, 1, 8, 0));
    let kyiv = Zone::named("Europe/Kiev").unwrap();
    assert_eq!(kyiv.from_utc(at(2026, 10, 22, 21, 30)), at(2026, 10, 23, 0, 30), "across midnight");
    assert_eq!(kyiv.offset_label(at(2026, 10, 22, 0, 0)), "UTC+03:00");
    assert_eq!(Zone::named("America/St_Johns").unwrap().offset_label(at(2026, 1, 1, 0, 0)), "UTC−03:30");
    assert_eq!(Zone::utc().offset_label(at(2026, 1, 1, 0, 0)), "UTC");
}

#[test]
fn skipped_and_repeated_times_follow_rfc_5545() {
    let lisbon = Zone::named("Europe/Lisbon").unwrap();
    // 2026-03-29 01:00 → 02:00 is skipped: 01:30 reads with the offset before the change.
    assert_eq!(lisbon.to_utc(at(2026, 3, 29, 1, 30)), at(2026, 3, 29, 1, 30));
    // 2026-10-25 01:30 happens twice: the first one (summer time) is meant.
    assert_eq!(lisbon.to_utc(at(2026, 10, 25, 1, 30)), at(2026, 10, 25, 0, 30));
}

#[test]
fn understands_windows_names_and_vendor_prefixes() {
    assert_eq!(Zone::named("GMT Standard Time").unwrap().name(), "Europe/London");
    assert_eq!(Zone::named("\"FLE Standard Time\"").unwrap().name(), "Europe/Kyiv");
    assert_eq!(Zone::named("w. europe standard time").unwrap().name(), "Europe/Berlin");
    assert_eq!(Zone::named("/mozilla.org/20050126_1/Europe/Madrid").unwrap().name(), "Europe/Madrid");
    assert_eq!(Zone::named("/citadel.org/20070227_1/America/Argentina/Buenos_Aires").unwrap().name(), "America/Argentina/Buenos_Aires");
    assert!(Zone::named("UTC").unwrap().is_utc());
    assert!(Zone::named("Etc/UTC").unwrap().is_utc());
    for bad in ["", "Mars/Olympus_Mons", "Customized Time Zone", "/", "//", "../../etc/passwd", "\u{0}", &"Europe/".repeat(100)] {
        assert!(Zone::named(bad).is_none(), "{bad:?}");
    }
    assert!(all_names().iter().any(|n| n == "Europe/Lisbon"));
}

#[test]
fn edges_of_time_do_not_panic() {
    let z = Zone::named("Pacific/Kiritimati").unwrap();
    for t in [at(1, 1, 1, 0, 0), at(9999, 12, 31, 23, 59), DateTime::new(Date::new(2016, 12, 31).unwrap(), 23, 59, 60).unwrap()] {
        let _ = z.from_utc(t);
        let _ = z.to_utc(t);
        let _ = z.offset_label(t);
    }
    // Past the end of time a conversion gives the input back.
    assert_eq!(z.from_utc(at(9999, 12, 31, 23, 0)), at(9999, 12, 31, 23, 0));
}
