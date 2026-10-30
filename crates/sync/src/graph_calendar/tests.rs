use super::*;
use crate::calendar::Known;
use ibercorreo_core::CalendarRemote;
use ibercorreo_dav::Response;

fn d(y: i32, m: u8, day: u8) -> Date {
    Date::new(y, m, day).unwrap()
}

fn at(y: i32, m: u8, day: u8, h: u8, min: u8) -> DateTime {
    DateTime::new(d(y, m, day), h, min, 0).unwrap()
}

/// Method, path, headers and body of a request.
type Asked = (String, String, Vec<(String, String)>, String);

/// Answers from routes (method, a piece of the path, status, body), the first that matches,
/// recording every request.
#[derive(Default)]
struct Fake {
    routes: Vec<(&'static str, String, u16, String)>,
    asked: Vec<Asked>,
}

impl Fake {
    fn on(mut self, method: &'static str, path: &str, status: u16, body: &str) -> Self {
        self.routes.push((method, path.to_string(), status, body.to_string()));
        self
    }
}

impl Http for Fake {
    fn send(&mut self, method: &str, path: &str, headers: &[(&str, &str)], body: &[u8]) -> Result<Response> {
        let headers = headers.iter().map(|(n, v)| (n.to_string(), v.to_string())).collect();
        self.asked.push((method.to_string(), path.to_string(), headers, String::from_utf8_lossy(body).into_owned()));
        let found = self.routes.iter().find(|(m, p, _, _)| *m == method && path.contains(p.as_str()));
        let (status, body) = found.map_or((404, String::new()), |(_, _, s, b)| (*s, b.clone()));
        Ok(Response { status, headers: Vec::new(), body: body.into_bytes(), close: false })
    }
}

fn graph(fake: Fake) -> Graph<Fake> {
    Graph::new(fake).without_sleeping()
}

#[test]
fn outlook_patterns_become_rrules_and_back() {
    let cases = [
        (
            json!({"pattern":{"type":"weekly","interval":2,"daysOfWeek":["monday","wednesday"],"firstDayOfWeek":"monday"},
                   "range":{"type":"endDate","startDate":"2026-11-02","endDate":"2026-12-31"}}),
            "FREQ=WEEKLY;INTERVAL=2;UNTIL=20261231;BYDAY=MO,WE",
        ),
        (
            json!({"pattern":{"type":"relativeMonthly","interval":1,"daysOfWeek":["friday"],"index":"last","firstDayOfWeek":"sunday"},
                   "range":{"type":"numbered","startDate":"2026-11-27","numberOfOccurrences":10}}),
            "FREQ=MONTHLY;COUNT=10;BYDAY=FR;BYSETPOS=-1;WKST=SU",
        ),
        (
            json!({"pattern":{"type":"absoluteYearly","interval":1,"month":3,"dayOfMonth":19,"firstDayOfWeek":"sunday"},"range":{"type":"noEnd","startDate":"2027-03-19"}}),
            "FREQ=YEARLY;BYMONTHDAY=19;BYMONTH=3;WKST=SU",
        ),
        (json!({"pattern":{"type":"daily","interval":3,"firstDayOfWeek":"sunday"},"range":{"type":"noEnd"}}), "FREQ=DAILY;INTERVAL=3;WKST=SU"),
    ];
    for (outlook, expected) in &cases {
        let text = rrule(outlook).unwrap();
        assert_eq!(text, *expected);
        // And back: the same pattern and range.
        let back = recurrence(&Rule::parse(&text).unwrap(), d(2026, 11, 2), "Romance Standard Time").unwrap();
        for key in ["type", "interval", "daysOfWeek", "index", "month", "dayOfMonth"] {
            assert_eq!(back["pattern"].get(key), outlook["pattern"].get(key), "{key} of {expected}");
        }
        assert_eq!(back["range"]["type"], outlook["range"]["type"], "{expected}");
        assert_eq!(back["range"]["recurrenceTimeZone"], "Romance Standard Time");
    }
    // `BYDAY=2TU` is Outlook's second Tuesday.
    let second = recurrence(&Rule::parse("FREQ=MONTHLY;BYDAY=2TU").unwrap(), d(2026, 11, 10), "UTC").unwrap();
    assert_eq!((second["pattern"]["type"].as_str(), second["pattern"]["index"].as_str()), (Some("relativeMonthly"), Some("second")));
    // What Outlook has no pattern for is said, not sent wrong.
    for unsupported in ["FREQ=YEARLY;BYMONTH=1,6;BYMONTHDAY=1", "FREQ=MONTHLY;BYMONTHDAY=-1", "FREQ=MONTHLY;BYDAY=1MO,3MO"] {
        assert!(recurrence(&Rule::parse(unsupported).unwrap(), d(2026, 11, 2), "UTC").is_err(), "{unsupported}");
    }
    assert_eq!(rrule(&json!({"pattern":{"type":"hourly"}})), None);
    assert_eq!(rrule(&json!({"pattern":{"type":"weekly","daysOfWeek":[]}})), None);
}

fn graph_event(extra: Value) -> Value {
    let mut v = json!({
        "id": "AAMk=", "iCalUId": "040000008200E", "subject": "Reunión", "type": "singleInstance",
        "body": {"contentType": "text", "content": "Orden del día\n"}, "location": {"displayName": "Sala 2"},
        "start": {"dateTime": "2026-11-02T08:00:00.0000000", "timeZone": "UTC"},
        "end": {"dateTime": "2026-11-02T09:00:00.0000000", "timeZone": "UTC"},
        "isAllDay": false, "isReminderOn": true, "reminderMinutesBeforeStart": 15,
        "originalStartTimeZone": "Romance Standard Time"
    });
    for (k, val) in extra.as_object().unwrap() {
        v[k] = val.clone();
    }
    v
}

#[test]
fn outlook_events_keep_their_own_zone() {
    let e = event(&graph_event(json!({})), 7).unwrap();
    assert_eq!(
        (e.uid.as_str(), e.summary.as_str(), e.location.as_str(), e.description.as_str()),
        ("040000008200E", "Reunión", "Sala 2", "Orden del día")
    );
    assert_eq!(e.when, When::timed(at(2026, 11, 2, 9, 0), at(2026, 11, 2, 10, 0), false).unwrap(), "08:00 UTC is 09:00 in Madrid");
    assert_eq!((e.tzid.as_deref(), e.calendar, e.reminders.as_slice()), (Some("Romance Standard Time"), 7, &[15][..]));
    // A zone Graph names its own way stays in UTC; so does one we do not know.
    for zone in ["tzone://Microsoft/Utc", "Customized Time Zone", "UTC"] {
        let e = event(&graph_event(json!({ "originalStartTimeZone": zone })), 7).unwrap();
        assert_eq!((e.when, e.tzid), (When::timed(at(2026, 11, 2, 8, 0), at(2026, 11, 2, 9, 0), true).unwrap(), None), "{zone}");
    }
    let all_day = graph_event(json!({
        "isAllDay": true, "isReminderOn": false,
        "start": {"dateTime": "2026-12-24T00:00:00.0000000", "timeZone": "UTC"}, "end": {"dateTime": "2026-12-26T00:00:00.0000000", "timeZone": "UTC"}
    }));
    let e = event(&all_day, 7).unwrap();
    assert_eq!((e.when, e.reminders.len()), (When::all_day(d(2026, 12, 24), d(2026, 12, 26)).unwrap(), 0));
    // Nothing usable: no event, no panic.
    for bad in [
        json!({}),
        json!({"id": "x", "start": {"dateTime": "mañana"}, "end": {}}),
        graph_event(json!({"end": {"dateTime": "2026-11-01T00:00:00", "timeZone": "UTC"}})),
    ] {
        assert!(event(&bad, 7).is_none(), "{bad}");
    }
}

fn weekly_master() -> Event {
    let v = graph_event(json!({
        "type": "seriesMaster",
        "recurrence": {"pattern": {"type": "weekly", "interval": 1, "daysOfWeek": ["monday"], "firstDayOfWeek": "monday"},
                       "range": {"type": "noEnd", "startDate": "2026-11-02"}}
    }));
    event(&v, 7).unwrap()
}

fn instance(kind: &str, original: &str, start: &str, end: &str) -> Value {
    json!({"id": format!("occ-{original}"), "type": kind, "subject": "Reunión (movida)", "originalStart": original,
           "start": {"dateTime": start, "timeZone": "UTC"}, "end": {"dateTime": end, "timeZone": "UTC"}, "iCalUId": "other"})
}

#[test]
fn deleted_and_changed_occurrences_come_from_the_instances() {
    let master = weekly_master();
    assert_eq!(master.rrule.as_deref(), Some("FREQ=WEEKLY;BYDAY=MO"));
    let found = vec![
        instance("occurrence", "2026-11-02T08:00:00Z", "2026-11-02T08:00:00", "2026-11-02T09:00:00"),
        // 11-09 was deleted in Outlook; 11-16 moved to 10:00 Madrid time.
        instance("exception", "2026-11-16T08:00:00Z", "2026-11-16T09:00:00", "2026-11-16T10:00:00"),
        instance("occurrence", "2026-11-23T08:00:00Z", "2026-11-23T08:00:00", "2026-11-23T09:00:00"),
    ];
    let events = with_instances(master, &found, d(2026, 11, 2), d(2026, 11, 23));
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].exdates, [d(2026, 11, 9)]);
    let moved = &events[1];
    assert_eq!((moved.uid.as_str(), moved.recurrence_id, moved.rrule.as_deref()), ("040000008200E", Some(d(2026, 11, 16)), None));
    assert_eq!(moved.when, When::timed(at(2026, 11, 16, 10, 0), at(2026, 11, 16, 11, 0), false).unwrap());
    assert_eq!(moved.tzid.as_deref(), Some("Romance Standard Time"));
    // Written out and read back, the series shows that week skipped and the moved one moved.
    let text = ibercorreo_ical::write_resource(events.iter(), "", at(2026, 10, 30, 12, 0));
    let parsed = ibercorreo_ical::parse(&text).unwrap();
    assert_eq!(parsed.events.len(), 2);
    let back: Vec<Event> = parsed.events.into_iter().map(|e| e.into_event(EventId(0), 7)).collect();
    assert!(!back[0].occurs_on(d(2026, 11, 9)) && back[0].occurs_on(d(2026, 11, 23)));
}

fn state_with(items: &[(&str, &str)], changes: Vec<Change>) -> State {
    let mut state = State::default();
    let known = Known { ctag: None, items: items.iter().map(|(h, e)| (h.to_string(), e.to_string())).collect() };
    state.calendars.insert("/v1.0/users/u1/calendars/AAA=/".into(), known);
    // A CalDAV calendar in the same store: never sent to Graph.
    state.calendars.insert("/dav/ana/work/".into(), Known::default());
    state.changes.push(Change::Delete { calendar: "/dav/ana/work/".into(), href: "/dav/ana/work/x.ics".into(), etag: "\"1\"".into() });
    state.changes.extend(changes);
    state
}

const CAL: &str = "/v1.0/users/u1/calendars/AAA=/";

#[test]
fn a_sync_converts_only_new_and_changed_events() {
    let events = json!({"value": [
        graph_event(json!({"id": "e1", "@odata.etag": "W/\"1\""})),
        graph_event(json!({"id": "e/2", "@odata.etag": "W/\"2\"", "subject": "Nueva"})),
    ]});
    let fake = Fake::default()
        .on("GET", "/v1.0/me?$select=id", 200, r#"{"id":"u1"}"#)
        .on("GET", "/v1.0/users/u1/calendars?", 200, r##"{"value":[{"id":"AAA=","name":"Calendario","hexColor":"#e3008c"}]}"##)
        .on("GET", "/calendars/AAA=/events?", 200, &events.to_string());
    let mut g = graph(fake);
    let state = state_with(&[(&format!("{CAL}events/e1"), "W/\"1\""), (&format!("{CAL}events/gone"), "W/\"9\"")], Vec::new());
    let batch = run(&mut g, &state, at(2026, 10, 30, 12, 0)).unwrap();
    assert_eq!(batch.scope.as_deref(), Some("/v1.0/users/u1/"));
    assert!(batch.pushed.is_empty() && batch.errors.is_empty(), "{batch:?}");
    let [cal] = batch.calendars.as_slice() else { panic!("{batch:?}") };
    assert_eq!((cal.calendar.href.as_str(), cal.calendar.name.as_str(), cal.calendar.color.as_deref()), (CAL, "Calendario", Some("#e3008c")));
    let hrefs: Vec<&str> = cal.fetched.iter().map(|f| f.href.as_str()).collect();
    assert_eq!(hrefs, [format!("{CAL}events/e%2F2")], "the unchanged one is not converted again; ids are path-safe");
    assert!(cal.fetched[0].data.contains("SUMMARY:Nueva") && cal.fetched[0].etag == "W/\"2\"");
    assert_eq!(cal.removed, [format!("{CAL}events/gone")]);
    assert!(g_asked(&g).iter().all(|(m, p)| m == "GET" && !p.contains("/dav/")), "{:?}", g_asked(&g));
    // Times in UTC and text bodies were asked for.
    let events_request = g.http().asked.iter().find(|a| a.1.contains("/events?")).unwrap();
    assert!(events_request.2.iter().any(|(n, v)| n == "Prefer" && v.contains("outlook.timezone=\"UTC\"")));
}

fn g_asked(g: &Graph<Fake>) -> Vec<(String, String)> {
    g.http().asked.iter().map(|a| (a.0.clone(), a.1.clone())).collect()
}

fn ics(e: &Event) -> String {
    ibercorreo_ical::write_resource([e], "", at(2026, 10, 30, 12, 0))
}

#[test]
fn changes_go_back_as_post_patch_and_delete() {
    let mut new = event(&graph_event(json!({})), 0).unwrap();
    new.uid = "local-1".into();
    new.reminders = vec![10];
    let mut changed = new.clone();
    changed.summary = "Cambiada".into();
    let changes = vec![
        Change::Put { calendar: CAL.into(), href: format!("{CAL}local-1.ics"), etag: None, uid: "local-1".into(), body: ics(&new) },
        Change::Put { calendar: CAL.into(), href: format!("{CAL}events/e1"), etag: Some("W/\"1\"".into()), uid: "u-e1".into(), body: ics(&changed) },
        Change::Put { calendar: CAL.into(), href: format!("{CAL}events/e2"), etag: Some("W/\"2\"".into()), uid: "u-e2".into(), body: ics(&changed) },
        Change::Delete { calendar: CAL.into(), href: format!("{CAL}events/e3"), etag: "W/\"3\"".into() },
        Change::Delete { calendar: CAL.into(), href: format!("{CAL}events/e4"), etag: "W/\"4\"".into() },
        // Another account's calendar: left for that account's sync.
        Change::Delete { calendar: "/v1.0/users/u2/calendars/B/".into(), href: "/v1.0/users/u2/calendars/B/events/x".into(), etag: "W/\"1\"".into() },
    ];
    let fake = Fake::default()
        .on("GET", "/v1.0/me?$select=id", 200, r#"{"id":"u1"}"#)
        .on("POST", "/calendars/AAA=/events", 201, r#"{"id":"NEW+1","@odata.etag":"W/\"7\""}"#)
        .on("PATCH", "/events/e1", 200, "{}")
        .on("PATCH", "/events/e2", 412, r#"{"error":{"code":"ErrorIrresolvableConflict"}}"#)
        .on("DELETE", "/events/e3", 204, "")
        .on("DELETE", "/events/e4", 404, "")
        .on("GET", "/v1.0/users/u1/calendars?", 200, r#"{"value":[]}"#);
    let mut g = graph(fake);
    let batch = run(&mut g, &state_with(&[], changes), at(2026, 10, 30, 12, 0)).unwrap();
    assert_eq!(
        batch.pushed,
        [
            Pushed::Put { calendar: CAL.into(), href: format!("{CAL}events/NEW%2B1"), uid: "local-1".into() },
            Pushed::Put { calendar: CAL.into(), href: format!("{CAL}events/e1"), uid: "u-e1".into() },
            Pushed::Conflict { href: format!("{CAL}events/e2") },
            Pushed::Deleted { href: format!("{CAL}events/e3") },
            Pushed::Deleted { href: format!("{CAL}events/e4") },
        ]
    );
    assert!(batch.errors.is_empty(), "{:?}", batch.errors);
    let asked = &g.http().asked;
    let post = asked.iter().find(|a| a.0 == "POST").unwrap();
    let sent: Value = serde_json::from_str(&post.3).unwrap();
    assert_eq!(sent["subject"], "Reunión");
    // A one-off appointment travels in UTC (exact anywhere); a series keeps its zone (below).
    assert_eq!(sent["start"], json!({"dateTime": "2026-11-02T08:00:00", "timeZone": "UTC"}));
    assert_eq!((sent["reminderMinutesBeforeStart"].as_i64(), sent["recurrence"].is_null()), (Some(10), true));
    let patch = asked.iter().find(|a| a.0 == "PATCH" && a.1.ends_with("/events/e1")).unwrap();
    assert!(patch.2.iter().any(|(n, v)| n == "If-Match" && v == "W/\"1\""), "only over the version last seen");
    assert!(patch.3.contains("Cambiada"));
    assert!(asked.iter().find(|a| a.0 == "DELETE").unwrap().2.iter().any(|(n, v)| n == "If-Match" && v == "W/\"3\""));
    assert!(!asked.iter().any(|a| a.1.contains("/dav/") || a.1.contains("/users/u2/")), "{asked:?}");
}

#[test]
fn a_deleted_occurrence_here_deletes_that_instance() {
    let mut master = weekly_master();
    master.uid = "u-s".into();
    master.exdates = vec![d(2026, 11, 9)];
    let found = json!({"value": [
        instance("occurrence", "2026-11-02T08:00:00Z", "2026-11-02T08:00:00", "2026-11-02T09:00:00"),
        instance("occurrence", "2026-11-09T08:00:00Z", "2026-11-09T08:00:00", "2026-11-09T09:00:00"),
    ]});
    let fake = Fake::default()
        .on("GET", "/v1.0/me?$select=id", 200, r#"{"id":"u1"}"#)
        .on("PATCH", "/calendars/AAA=/events/s1", 200, "{}")
        .on("GET", "/v1.0/users/u1/events/s1/instances?", 200, &found.to_string())
        .on("DELETE", "/v1.0/users/u1/events/occ-2026-11-09T08%3A00%3A00Z", 204, "")
        .on("GET", "/v1.0/users/u1/calendars?", 200, r#"{"value":[]}"#);
    let mut g = graph(fake);
    let change =
        Change::Put { calendar: CAL.into(), href: format!("{CAL}events/s1"), etag: Some("W/\"1\"".into()), uid: "u-s".into(), body: ics(&master) };
    let batch = run(&mut g, &state_with(&[], vec![change]), at(2026, 10, 30, 12, 0)).unwrap();
    assert!(batch.errors.is_empty(), "{:?}", batch.errors);
    let asked = g_asked(&g);
    assert!(asked.contains(&("DELETE".into(), "/v1.0/users/u1/events/occ-2026-11-09T08%3A00%3A00Z".into())), "{asked:?}");
    assert_eq!(asked.iter().filter(|(m, _)| m == "DELETE").count(), 1, "only the deleted week");
    let patch = g.http().asked.iter().find(|a| a.0 == "PATCH").unwrap();
    let sent: Value = serde_json::from_str(&patch.3).unwrap();
    assert_eq!(sent["recurrence"]["pattern"]["daysOfWeek"], json!(["monday"]));
    assert_eq!(sent["start"], json!({"dateTime": "2026-11-02T09:00:00", "timeZone": "Romance Standard Time"}));
}

#[test]
fn graph_and_caldav_calendars_never_remove_each_other() {
    let mut store = ibercorreo_store::Store::new();
    let caldav = store.add_calendar("Trabajo", "#112233", Some(CalendarRemote { href: "/dav/ana/work/".into(), ctag: None })).unwrap();
    let mine = store.add_calendar("Calendario", "#445566", Some(CalendarRemote { href: CAL.into(), ctag: None })).unwrap();
    let other = store.add_calendar("Otro", "#778899", Some(CalendarRemote { href: "/v1.0/users/u2/calendars/B/".into(), ctag: None })).unwrap();
    // u1's Graph sync no longer lists its calendar: only that one goes.
    let report = crate::calendar::apply(&mut store, Batch { scope: Some("/v1.0/users/u1/".into()), ..Batch::default() });
    assert_eq!(report.calendars_removed, 1);
    let ids: Vec<u32> = store.calendars().iter().map(|c| c.id).collect();
    assert!(ids.contains(&caldav) && ids.contains(&other) && !ids.contains(&mine), "{ids:?}");
    // A CalDAV sync listing nothing removes only the CalDAV one.
    let report = crate::calendar::apply(&mut store, Batch::default());
    assert_eq!(report.calendars_removed, 1);
    assert!(store.calendars().iter().any(|c| c.id == other));
    // Forgetting the CalDAV address keeps the Outlook calendars.
    assert_eq!(crate::calendar::forget(&mut store).calendars_removed, 0);
    assert!(store.calendars().iter().any(|c| c.id == other));
}
