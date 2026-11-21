//! Replays events at their recorded pace, scaled, or as fast as possible.

use std::time::Duration;

use thiserror::Error;

use crate::event::Event;

/// How fast to replay a session.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Pace {
    /// Wait out the recorded gaps between events, divided by `speed`.
    Recorded {
        /// How many times faster than recorded; 1.0 is real time.
        speed: f64,
    },
    /// Never wait.
    Fast,
}

/// A replay speed that is not a positive, finite number.
#[derive(Debug, Error)]
#[error("replay speed must be a positive number, not {0}")]
pub struct InvalidSpeed(pub f64);

impl Pace {
    /// Recorded pace at `speed` times real time.
    pub fn recorded(speed: f64) -> Result<Self, InvalidSpeed> {
        if speed.is_finite() && speed > 0.0 {
            Ok(Self::Recorded { speed })
        } else {
            Err(InvalidSpeed(speed))
        }
    }

    /// How long to wait for an event recorded `gap_ms` after the previous one.
    fn wait(self, gap_ms: u64) -> Option<Duration> {
        match self {
            Self::Fast => None,
            Self::Recorded { .. } if gap_ms == 0 => None,
            Self::Recorded { speed } => Some(
                Duration::try_from_secs_f64(gap_ms as f64 / 1000.0 / speed)
                    .unwrap_or(Duration::MAX),
            ),
        }
    }
}

/// An iterator that yields `events` after sleeping out their recorded gaps.
///
/// The first gap is measured from session start (`t = 0`). An event stamped
/// earlier than its predecessor plays at once. Errors pass through without
/// waiting. `sleep` is `std::thread::sleep` in the binary and a recorder in
/// tests.
pub struct Replay<I, S> {
    events: I,
    pace: Pace,
    sleep: S,
    clock_ms: u64,
}

impl<I, S> Replay<I, S> {
    /// A replay of `events` at `pace`, waiting with `sleep`.
    pub fn new(events: I, pace: Pace, sleep: S) -> Self {
        Self {
            events,
            pace,
            sleep,
            clock_ms: 0,
        }
    }
}

impl<I, E, S> Iterator for Replay<I, S>
where
    I: Iterator<Item = Result<Event, E>>,
    S: FnMut(Duration),
{
    type Item = Result<Event, E>;

    fn next(&mut self) -> Option<Self::Item> {
        let item = self.events.next()?;
        if let Ok(event) = &item {
            if let Some(wait) = self.pace.wait(event.t.saturating_sub(self.clock_ms)) {
                (self.sleep)(wait);
            }
            self.clock_ms = self.clock_ms.max(event.t);
        }
        Some(item)
    }
}

#[cfg(test)]
mod tests {
    use std::convert::Infallible;

    use super::*;
    use crate::event::EventKind;

    fn at(times: &[u64]) -> impl Iterator<Item = Result<Event, Infallible>> + '_ {
        times
            .iter()
            .map(|&t| Ok(Event::new(t, EventKind::TurnEnd { turn: 1 })))
    }

    fn sleeps(times: &[u64], pace: Pace) -> Vec<Duration> {
        let mut slept = Vec::new();
        let played = Replay::new(at(times), pace, |d| slept.push(d)).count();
        assert_eq!(played, times.len());
        slept
    }

    #[test]
    fn recorded_pace_sleeps_the_gaps_between_events() {
        assert_eq!(
            sleeps(&[250, 250, 1000, 1500], Pace::recorded(1.0).unwrap()),
            [
                Duration::from_millis(250),
                Duration::from_millis(750),
                Duration::from_millis(500),
            ]
        );
    }

    #[test]
    fn speed_divides_the_gaps() {
        assert_eq!(
            sleeps(&[0, 1000, 3000], Pace::recorded(4.0).unwrap()),
            [Duration::from_millis(250), Duration::from_millis(500)]
        );
        assert_eq!(
            sleeps(&[0, 100], Pace::recorded(0.5).unwrap()),
            [Duration::from_millis(200)]
        );
    }

    #[test]
    fn fast_pace_never_sleeps() {
        assert!(sleeps(&[0, 1000, 60_000, 7_200_000], Pace::Fast).is_empty());
    }

    #[test]
    fn an_out_of_order_event_plays_at_once_and_does_not_rewind_the_clock() {
        assert_eq!(
            sleeps(&[1000, 400, 1500], Pace::recorded(1.0).unwrap()),
            [Duration::from_millis(1000), Duration::from_millis(500)]
        );
    }

    #[test]
    fn errors_pass_through_without_sleeping() {
        let items = vec![
            Err("bad"),
            Ok(Event::new(100, EventKind::TurnEnd { turn: 1 })),
        ];
        let mut slept = Vec::new();
        let out: Vec<_> = Replay::new(items.into_iter(), Pace::recorded(1.0).unwrap(), |d| {
            slept.push(d)
        })
        .collect();
        assert_eq!(out.len(), 2);
        assert!(out[0].is_err());
        assert_eq!(slept, [Duration::from_millis(100)]);
    }

    #[test]
    fn speed_must_be_positive_and_finite() {
        for bad in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert!(Pace::recorded(bad).is_err(), "{bad}");
        }
    }
}
