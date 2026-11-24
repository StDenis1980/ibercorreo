//! Dragging appointments in the day and week views (Tinta), as in Outlook: drag one to move it
//! (to another time, or another day of the week shown), or drag its bottom edge to make it longer
//! or shorter. Times snap to 15 minutes; the result goes through `event.move`, so it can be
//! undone like any edit. Repeating and all-day appointments do not drag yet (the engine says why).

use ibercorreo_core::{Date, DateTime, EventId};

/// What the pointer is doing to an appointment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    /// The whole appointment, keeping its length.
    Move,
    /// Its end only.
    Resize,
}

/// An appointment being dragged.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Drag {
    pub id: EventId,
    pub mode: Mode,
    /// Its start and end when the drag began, as the calendar shows them.
    pub start: DateTime,
    pub end: DateTime,
    /// Where the pointer was pressed.
    pub origin: egui::Pos2,
}

/// Tinta: an appointment dragged to another day: an all-day event in the strip above the hours,
/// or any appointment that does not repeat in the month view. It keeps its length and, when it
/// has one, its time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DayDrag {
    pub id: EventId,
    /// The day it was picked up on.
    pub from: Date,
}

/// The step a drag snaps to, in minutes.
pub(crate) const SNAP: i64 = 15;
/// The shortest an appointment is made by dragging its edge.
const MIN_LENGTH: i64 = SNAP;

fn minutes(t: DateTime) -> i64 {
    t.to_unix().div_euclid(60)
}

fn at(minutes: i64) -> Option<DateTime> {
    DateTime::from_unix(minutes.checked_mul(60)?).ok()
}

/// How far the pointer moved, `(dx, dy)` in points, as days and minutes for a grid whose columns
/// are `col_w` wide and whose hours are `hour_h` high: whole days, and minutes snapped to [`SNAP`].
pub(crate) fn offset(dx: f32, dy: f32, col_w: f32, hour_h: f32) -> (i64, i64) {
    if !(dx.is_finite() && dy.is_finite() && col_w > 0.0 && hour_h > 0.0) {
        return (0, 0);
    }
    let days = (dx / col_w).round().clamp(-366.0, 366.0) as i64;
    let steps = (dy / hour_h * 60.0 / SNAP as f32).round().clamp(-10_000.0, 10_000.0) as i64;
    (days, steps.saturating_mul(SNAP))
}

impl Drag {
    /// Where the appointment goes for a pointer moved by `(days, minutes)`: its new start and end,
    /// as the calendar shows them. A resize never makes it shorter than [`SNAP`] and ignores days
    /// (the end stays on its own column).
    pub(crate) fn target(&self, days: i64, mins: i64) -> Option<(DateTime, DateTime)> {
        let (s, e) = (minutes(self.start), minutes(self.end));
        match self.mode {
            Mode::Move => {
                let shift = days.checked_mul(24 * 60)?.checked_add(mins)?;
                Some((at(s.checked_add(shift)?)?, at(e.checked_add(shift)?)?))
            }
            Mode::Resize => {
                let end = e.checked_add(mins)?.max(s.checked_add(MIN_LENGTH)?);
                Some((self.start, at(end)?))
            }
        }
    }
}

/// `t` as `event.move` takes it: `YYYY-MM-DDTHH:MM`.
pub(crate) fn param(t: DateTime) -> String {
    format!("{}T{:02}:{:02}", t.date, t.hour(), t.minute())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(day: u8, h: u8, m: u8) -> DateTime {
        DateTime::new(Date::new(2026, 11, day).unwrap(), h, m, 0).unwrap()
    }

    #[test]
    fn a_drag_snaps_to_quarter_hours_and_whole_days() {
        assert_eq!(offset(0.0, 52.0, 100.0, 52.0), (0, 60));
        assert_eq!(offset(0.0, 10.0, 100.0, 52.0), (0, 15), "10 pt of a 52 pt hour is about 12 minutes: a quarter");
        assert_eq!(offset(0.0, 5.0, 100.0, 52.0), (0, 0));
        assert_eq!(offset(160.0, -26.0, 100.0, 52.0), (2, -30));
        assert_eq!(offset(f32::NAN, 1e30, 0.0, 52.0), (0, 0), "hostile numbers do nothing");
        assert_eq!(offset(1e30, 1e30, 1.0, 1.0).0, 366);
    }

    #[test]
    fn moving_keeps_the_length_and_resizing_keeps_a_quarter() {
        let d = Drag { id: EventId(1), mode: Mode::Move, start: t(24, 10, 0), end: t(24, 11, 30), origin: egui::Pos2::ZERO };
        assert_eq!(d.target(1, 45), Some((t(25, 10, 45), t(25, 12, 15))));
        assert_eq!(d.target(0, -10 * 60 - 15), Some((t(23, 23, 45), t(24, 1, 15))), "across midnight");
        let r = Drag { mode: Mode::Resize, ..d };
        assert_eq!(r.target(3, 30), Some((t(24, 10, 0), t(24, 12, 0))), "days are ignored");
        assert_eq!(r.target(0, -600), Some((t(24, 10, 0), t(24, 10, 15))));
        assert_eq!(d.target(i64::MAX, 0), None);
        assert_eq!(param(t(5, 9, 5)), "2026-11-05T09:05");
    }
}
