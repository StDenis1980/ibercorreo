use super::*;

fn d(y: i32, m: u8, day: u8) -> Date {
    Date::new(y, m, day).unwrap()
}

fn days(rule: &str, start: Date, from: Date, to: Date) -> Vec<String> {
    Rule::parse(rule).unwrap().occurrences(start, 9 * 60, from, to, &[]).iter().map(ToString::to_string).collect()
}

#[test]
fn daily_weekly_with_interval_count_until() {
    let start = d(2026, 10, 22); // a Thursday
    assert_eq!(days("FREQ=DAILY;COUNT=3", start, start, d(2026, 12, 31)), ["2026-10-22", "2026-10-23", "2026-10-24"]);
    assert_eq!(days("FREQ=DAILY;INTERVAL=10;UNTIL=20261111", start, start, d(2026, 12, 31)), ["2026-10-22", "2026-11-01", "2026-11-11"]);
    assert_eq!(days("FREQ=WEEKLY;BYDAY=MO,TH;COUNT=4", start, start, d(2026, 12, 31)), ["2026-10-22", "2026-10-26", "2026-10-29", "2026-11-02"]);
    assert_eq!(days("FREQ=WEEKLY;INTERVAL=2", start, start, d(2026, 11, 30)), ["2026-10-22", "2026-11-05", "2026-11-19"]);
    // UNTIL with a time before the start time excludes that day.
    assert_eq!(days("FREQ=DAILY;UNTIL=20261024T080000Z", start, start, d(2026, 12, 31)), ["2026-10-22", "2026-10-23"]);
    assert_eq!(
        days("FREQ=DAILY;BYDAY=MO,TU,WE,TH,FR", start, start, d(2026, 10, 28)),
        ["2026-10-22", "2026-10-23", "2026-10-26", "2026-10-27", "2026-10-28"]
    );
}

#[test]
fn monthly_and_yearly_shapes() {
    // The 31st: months without one are skipped, not moved.
    assert_eq!(days("FREQ=MONTHLY;COUNT=3", d(2026, 1, 31), d(2026, 1, 1), d(2026, 12, 31)), ["2026-01-31", "2026-03-31", "2026-05-31"]);
    assert_eq!(
        days("FREQ=MONTHLY;BYMONTHDAY=-1;COUNT=3", d(2026, 1, 31), d(2026, 1, 1), d(2026, 12, 31)),
        ["2026-01-31", "2026-02-28", "2026-03-31"]
    );
    // Second Tuesday, last Friday.
    assert_eq!(days("FREQ=MONTHLY;BYDAY=2TU;COUNT=3", d(2026, 10, 13), d(2026, 1, 1), d(2027, 12, 31)), ["2026-10-13", "2026-11-10", "2026-12-08"]);
    assert_eq!(days("FREQ=MONTHLY;BYDAY=-1FR;COUNT=2", d(2026, 10, 30), d(2026, 1, 1), d(2027, 12, 31)), ["2026-10-30", "2026-11-27"]);
    // Last weekday of the month.
    assert_eq!(
        days("FREQ=MONTHLY;BYDAY=MO,TU,WE,TH,FR;BYSETPOS=-1;COUNT=3", d(2026, 10, 30), d(2026, 1, 1), d(2027, 12, 31)),
        ["2026-10-30", "2026-11-30", "2026-12-31"]
    );
    // A birthday on 29 February shows only in leap years.
    assert_eq!(days("FREQ=YEARLY;COUNT=2", d(2024, 2, 29), d(2024, 1, 1), d(2036, 12, 31)), ["2024-02-29", "2028-02-29"]);
    // US Thanksgiving: fourth Thursday of November.
    assert_eq!(
        days("FREQ=YEARLY;BYMONTH=11;BYDAY=4TH;COUNT=3", d(2026, 11, 26), d(2026, 1, 1), d(2030, 1, 1)),
        ["2026-11-26", "2027-11-25", "2028-11-23"]
    );
    assert_eq!(days("FREQ=YEARLY;BYDAY=1MO;COUNT=2", d(2026, 1, 5), d(2026, 1, 1), d(2030, 1, 1)), ["2026-01-05", "2027-01-04"]);
}

#[test]
fn queries_far_from_the_start_are_cheap_and_right() {
    let start = d(2000, 1, 3); // a Monday
    assert_eq!(days("FREQ=WEEKLY;BYDAY=MO", start, d(2026, 10, 22), d(2026, 10, 31)), ["2026-10-26"]);
    assert_eq!(days("FREQ=DAILY;INTERVAL=3", start, d(2026, 10, 22), d(2026, 10, 25)), ["2026-10-22", "2026-10-25"]);
    assert_eq!(days("FREQ=MONTHLY;BYMONTHDAY=15", start, d(2026, 10, 1), d(2026, 11, 30)), ["2026-10-15", "2026-11-15"]);
    // With COUNT the walk starts at the beginning, so the count is right.
    assert_eq!(days("FREQ=DAILY;COUNT=5", start, d(2000, 1, 6), d(2026, 1, 1)), ["2000-01-06", "2000-01-07"]);
    let excluded = [d(2026, 10, 27)];
    let r = Rule::parse("FREQ=DAILY").unwrap().occurrences(d(2026, 10, 26), 0, d(2026, 10, 26), d(2026, 10, 28), &excluded);
    assert_eq!(r, [d(2026, 10, 26), d(2026, 10, 28)]);
}

#[test]
fn unsupported_and_hostile_rules_fail_cleanly() {
    for bad in [
        "",
        "COUNT=3",
        "FREQ=HOURLY",
        "FREQ=SECONDLY",
        "FREQ=DAILY;BYHOUR=9",
        "FREQ=YEARLY;BYWEEKNO=20",
        "FREQ=DAILY;INTERVAL=0",
        "FREQ=DAILY;INTERVAL=-1",
        "FREQ=WEEKLY;BYDAY=1MO",
        "FREQ=MONTHLY;BYDAY=0MO",
        "FREQ=MONTHLY;BYDAY=XX",
        "FREQ=MONTHLY;BYMONTHDAY=32",
        "FREQ=MONTHLY;BYMONTH=13",
        "FREQ=DAILY;UNTIL=2026",
        "FREQ=DAILY;UNTIL=20261022T2599",
        "FREQ=DAILY;COUNT=x",
        "FREQ",
        "FREQ=MONTHLY;BYDAY=é",
    ] {
        assert!(Rule::parse(bad).is_err(), "{bad:?}");
    }
    assert!(Rule::parse(&format!("FREQ=DAILY;BYMONTH={}", vec!["1"; 100].join(","))).is_err());
    // A rule that never matches stops at the bound, not in a loop.
    let never = Rule::parse("FREQ=YEARLY;BYMONTH=2;BYMONTHDAY=30").unwrap();
    assert_eq!(never.occurrences(d(2026, 1, 1), 0, d(2026, 1, 2), d(9999, 12, 31), &[]), Vec::<Date>::new());
    let huge = Rule::parse("FREQ=DAILY;COUNT=4294967295").unwrap();
    assert_eq!(huge.count, Some(MAX_COUNT));
}

#[test]
fn text_round_trips() {
    for text in
        ["FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,WE", "FREQ=MONTHLY;COUNT=5;BYDAY=-1FR", "FREQ=YEARLY;UNTIL=20301231T235900;BYDAY=4TH;BYMONTH=11;WKST=SU"]
    {
        assert_eq!(Rule::parse(text).unwrap().to_text(), text);
    }
}
