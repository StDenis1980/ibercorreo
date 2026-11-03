use super::*;

/// Minutes from 1601-01-01 to midnight of `d`.
fn minutes(y: i32, m: u8, d: u8) -> u32 {
    let unix = DateTime::midnight(Date::new(y, m, d).unwrap()).to_unix();
    u32::try_from((unix + 11_644_473_600) / 60).unwrap()
}

struct P {
    frequency: u16,
    pattern: u16,
    period: u32,
    specific: Vec<u32>,
    end_type: u32,
    count: u32,
    first_dow: u32,
    deleted: Vec<u32>,
    changed: Vec<u32>,
    start: u32,
    end: u32,
}

impl Default for P {
    fn default() -> Self {
        P {
            frequency: DAILY,
            pattern: DAY,
            period: 1440,
            specific: vec![],
            end_type: 0x2023,
            count: 10,
            first_dow: 0,
            deleted: vec![],
            changed: vec![],
            start: minutes(2025, 3, 3),
            end: 0x5ae9_80df,
        }
    }
}

fn blob(p: &P) -> Vec<u8> {
    let mut b = Vec::new();
    for v in [0x3004u16, 0x3004, p.frequency, p.pattern, 0] {
        b.extend_from_slice(&v.to_le_bytes());
    }
    let mut words = vec![p.start, p.period, 0];
    words.extend(&p.specific);
    words.extend([p.end_type, p.count, p.first_dow, p.deleted.len() as u32]);
    words.extend(&p.deleted);
    words.push(p.changed.len() as u32);
    words.extend(&p.changed);
    words.extend([p.start, p.end]);
    for w in words {
        b.extend_from_slice(&w.to_le_bytes());
    }
    // The appointment part that follows (versions, times, exceptions) is not read.
    b.extend_from_slice(&[0x06, 0x30, 0, 0, 0x09, 0x30, 0, 0]);
    b
}

fn rule(p: P) -> String {
    read(&blob(&p)).unwrap().rrule
}

#[test]
fn patterns_become_rules() {
    assert_eq!(rule(P { period: 2880, end_type: END_AFTER_COUNT, count: 10, ..P::default() }), "FREQ=DAILY;INTERVAL=2;COUNT=10");
    assert_eq!(
        rule(P {
            frequency: WEEKLY,
            pattern: WEEK,
            period: 2,
            specific: vec![0x0a],
            end_type: END_AFTER_DATE,
            end: minutes(2025, 6, 30),
            first_dow: 1,
            ..P::default()
        }),
        "FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,WE;UNTIL=20250630;WKST=MO"
    );
    assert_eq!(
        rule(P { pattern: WEEK, period: 1, specific: vec![0x3e], ..P::default() }),
        "FREQ=WEEKLY;INTERVAL=1;BYDAY=MO,TU,WE,TH,FR",
        "every weekday"
    );
    assert_eq!(
        rule(P { frequency: MONTHLY, pattern: MONTH, period: 1, specific: vec![15], ..P::default() }),
        "FREQ=MONTHLY;INTERVAL=1;BYMONTHDAY=15"
    );
    assert_eq!(
        rule(P { frequency: MONTHLY, pattern: MONTH_END, period: 3, specific: vec![31], ..P::default() }),
        "FREQ=MONTHLY;INTERVAL=3;BYMONTHDAY=-1"
    );
    assert_eq!(
        rule(P { frequency: MONTHLY, pattern: MONTH_NTH, period: 1, specific: vec![0x20, 5], ..P::default() }),
        "FREQ=MONTHLY;INTERVAL=1;BYDAY=FR;BYSETPOS=-1",
        "the last Friday"
    );
    assert_eq!(
        rule(P { frequency: MONTHLY, pattern: MONTH_NTH, period: 2, specific: vec![0x41, 1], ..P::default() }),
        "FREQ=MONTHLY;INTERVAL=2;BYDAY=SU,SA;BYSETPOS=1"
    );
    assert_eq!(
        rule(P { frequency: YEARLY, pattern: MONTH, period: 12, specific: vec![8], start: minutes(2025, 3, 8), ..P::default() }),
        "FREQ=YEARLY;INTERVAL=1;BYMONTH=3;BYMONTHDAY=8"
    );
    assert_eq!(
        rule(P { frequency: YEARLY, pattern: MONTH_NTH, period: 24, specific: vec![0x10, 2], start: minutes(2025, 11, 13), ..P::default() }),
        "FREQ=YEARLY;INTERVAL=2;BYMONTH=11;BYDAY=TH;BYSETPOS=2"
    );
    assert_eq!(
        rule(P { frequency: YEARLY, pattern: MONTH_END, period: 12, specific: vec![31], start: minutes(2025, 2, 28), ..P::default() }),
        "FREQ=YEARLY;INTERVAL=1;BYMONTH=2;BYMONTHDAY=-1"
    );
}

#[test]
fn every_rule_is_one_the_calendar_reads() {
    for p in [
        P { period: 2880, end_type: END_AFTER_COUNT, ..P::default() },
        P {
            frequency: WEEKLY,
            pattern: WEEK,
            period: 2,
            specific: vec![0x0a],
            end_type: END_AFTER_DATE,
            end: minutes(2025, 6, 30),
            first_dow: 1,
            ..P::default()
        },
        P { frequency: MONTHLY, pattern: MONTH_NTH, period: 1, specific: vec![0x20, 5], ..P::default() },
        P { frequency: YEARLY, pattern: MONTH_NTH, period: 24, specific: vec![0x10, 2], ..P::default() },
    ] {
        let r = rule(p);
        assert!(ibercorreo_core::recur::Rule::parse(&r).is_ok(), "{r}");
    }
}

#[test]
fn deleted_and_changed_days() {
    let p = P {
        frequency: WEEKLY,
        pattern: WEEK,
        period: 1,
        specific: vec![0x02],
        deleted: vec![minutes(2025, 3, 10), minutes(2025, 3, 17), minutes(2025, 3, 24)],
        changed: vec![minutes(2025, 3, 17)],
        ..P::default()
    };
    let s = read(&blob(&p)).unwrap();
    let d = |m, d| Date::new(2025, m, d).unwrap();
    assert_eq!(s.deleted, [d(3, 10), d(3, 24)], "a changed day is replaced, not deleted");
    assert_eq!(s.changed, [d(3, 17)]);
}

#[test]
fn unreadable_patterns_give_none() {
    assert_eq!(read(&blob(&P { pattern: 0x000a, specific: vec![1], ..P::default() })), None, "Hijri");
    assert_eq!(read(&blob(&P { frequency: WEEKLY, pattern: WEEK, specific: vec![0], ..P::default() })), None, "no day");
    assert_eq!(read(&blob(&P { frequency: 0x1234, ..P::default() })), None);
    let good = blob(&P::default());
    for cut in 0..good.len() - 8 {
        assert_eq!(read(&good[..cut]), None, "cut at {cut}");
    }
    let mut huge = blob(&P::default());
    // The deleted-day count: after the versions, kinds, three words and three more.
    huge[10 + 4 * 6..10 + 4 * 7].copy_from_slice(&u32::MAX.to_le_bytes());
    assert_eq!(read(&huge), None);
    let mut x = 0x2545_f491u32;
    for _ in 0..2000 {
        let mut b = blob(&P { frequency: MONTHLY, pattern: MONTH_NTH, specific: vec![0x3e, 5], deleted: vec![minutes(2025, 4, 1)], ..P::default() });
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        let at = x as usize % b.len();
        b[at] = (x >> 8) as u8;
        let _ = read(&b);
    }
}
