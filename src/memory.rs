//! Long-term memory: deterministic counters and records, no LLM involved.
//! It stores kinds, counts, exit codes and durations, never command lines.
//! `observe` is pure: all clocks come in through `Obs`, all I/O lives elsewhere.

use std::collections::BTreeMap;

use crate::command::{Danger, Tool};
use crate::util::human_secs;

const DAY: i64 = 86_400;
const AWAY_HOURS: u64 = 4;

/// One finished command, as the memory sees it.
pub(crate) struct Obs {
    pub(crate) now: u64,
    pub(crate) offset: i64, // seconds east of UTC, so "today" means the user's today
    pub(crate) exit: i32,
    pub(crate) secs: u64,
    pub(crate) streak: u32, // consecutive failures in this shell, including this one
    pub(crate) tool: Option<Tool>,
    pub(crate) danger: Option<Danger>,
}

/// Something worth the cat remarking on. Each one fires on an edge (a milestone, a record,
/// a return), so none of them repeats on its own.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Fact {
    StreakRecord { n: u32 },
    SlowRecord { secs: u64 },
    DangerAgain { danger: Danger, n: u32 },
    NthFail { tool: Tool, n: u32 },
    FailsToday { n: u32 },
    BackAfterDays { days: i64 },
    BackAfterHours { hours: u64 },
    FirstToday,
    FirstEver,
}

fn nth(n: u32) -> String {
    let suffix = match (n % 10, n % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{n}{suffix}")
}

impl Fact {
    /// Lower sorts first: the most remarkable fact leads.
    fn rank(&self) -> u8 {
        match self {
            Fact::StreakRecord { .. } => 0,
            Fact::SlowRecord { .. } => 1,
            Fact::DangerAgain { .. } => 2,
            Fact::NthFail { .. } => 3,
            Fact::FailsToday { .. } => 4,
            Fact::BackAfterDays { .. } => 5,
            Fact::BackAfterHours { .. } => 6,
            Fact::FirstToday => 7,
            Fact::FirstEver => 8,
        }
    }

    /// A plain English sentence for the LLM prompt.
    pub(crate) fn note(&self) -> String {
        match self {
            Fact::StreakRecord { n } => format!("new personal record: {n} failed commands in a row"),
            Fact::SlowRecord { secs } => {
                format!("new record for the slowest command: {}", human_secs(*secs))
            }
            Fact::DangerAgain { danger, n } => format!(
                "this is the {} risky '{}' command today",
                nth(*n),
                danger.name()
            ),
            Fact::NthFail { tool, n } => {
                format!("the user's {} failed '{}' command today", nth(*n), tool.name())
            }
            Fact::FailsToday { n } => format!("{n} failed commands so far today"),
            Fact::BackAfterDays { days } => format!("the user is back after {days} days away"),
            Fact::BackAfterHours { hours } => format!("the user is back after {hours} hours away"),
            Fact::FirstToday => "this is the first command of the day".to_string(),
            Fact::FirstEver => "this is the first time you ever see this user".to_string(),
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq)]
pub(crate) struct Memory {
    last_ts: u64,
    days_seen: u32,
    max_streak: u32,
    max_secs: u64,
    day: i64, // the local day the counters below belong to
    fails: u32,
    tool_fails: BTreeMap<String, u32>,
    danger_runs: BTreeMap<String, u32>,
}

fn day_of(ts: u64, offset: i64) -> i64 {
    (ts as i64 + offset).div_euclid(DAY)
}

fn milestone(n: u32) -> bool {
    n == 3 || n == 5 || n == 8 || (n >= 10 && n % 10 == 0)
}

fn milestone_total(n: u32) -> bool {
    matches!(n, 10 | 25 | 50 | 100)
}

impl Memory {
    pub(crate) fn days_seen(&self) -> u32 {
        self.days_seen
    }

    /// Record one command and return the facts it triggered, most remarkable first.
    pub(crate) fn observe(&mut self, o: &Obs) -> Vec<Fact> {
        let mut facts = Vec::new();
        let day = day_of(o.now, o.offset);
        let first_ever = self.last_ts == 0;
        let away = o.now.saturating_sub(self.last_ts);

        if day != self.day {
            let days_away = if first_ever {
                0
            } else {
                (day - day_of(self.last_ts, o.offset)).max(0)
            };
            self.day = day;
            self.fails = 0;
            self.tool_fails.clear();
            self.danger_runs.clear();
            self.days_seen += 1;
            if first_ever {
                facts.push(Fact::FirstEver);
            } else if days_away >= 2 {
                facts.push(Fact::BackAfterDays { days: days_away });
            } else {
                facts.push(Fact::FirstToday);
            }
        } else if !first_ever && away >= AWAY_HOURS * 3600 {
            facts.push(Fact::BackAfterHours { hours: away / 3600 });
        }

        let failed = o.exit != 0 && o.exit != 130;
        if failed {
            self.fails += 1;
            if milestone_total(self.fails) {
                facts.push(Fact::FailsToday { n: self.fails });
            }
            if let Some(t) = o.tool {
                let slot = self.tool_fails.entry(t.name().to_string()).or_insert(0);
                *slot += 1;
                let n = *slot;
                if milestone(n) {
                    facts.push(Fact::NthFail { tool: t, n });
                }
            }
        }

        if let Some(d) = o.danger {
            let slot = self.danger_runs.entry(d.name().to_string()).or_insert(0);
            *slot += 1;
            let n = *slot;
            if n >= 2 && (n <= 3 || n % 5 == 0) {
                facts.push(Fact::DangerAgain { danger: d, n });
            }
        }

        if o.streak >= 4 && o.streak > self.max_streak {
            facts.push(Fact::StreakRecord { n: o.streak });
        }
        self.max_streak = self.max_streak.max(o.streak);

        // Editors, ssh sessions and the like are long because the user is busy in them.
        let interactive = o.tool.is_some_and(Tool::is_interactive);
        if !interactive {
            if o.secs >= 60 && self.max_secs > 0 && o.secs > self.max_secs {
                facts.push(Fact::SlowRecord { secs: o.secs });
            }
            self.max_secs = self.max_secs.max(o.secs);
        }

        self.last_ts = o.now;
        facts.sort_by_key(|f| f.rank());
        facts
    }

    pub(crate) fn to_text(&self) -> String {
        let mut s = format!(
            "v=1\nlast_ts={}\ndays_seen={}\nmax_streak={}\nmax_secs={}\nday={}\nfails={}\n",
            self.last_ts, self.days_seen, self.max_streak, self.max_secs, self.day, self.fails
        );
        for (k, v) in &self.tool_fails {
            s.push_str(&format!("tf.{k}={v}\n"));
        }
        for (k, v) in &self.danger_runs {
            s.push_str(&format!("dr.{k}={v}\n"));
        }
        s
    }

    /// Tolerant on purpose: unknown keys and bad numbers are skipped, never fatal.
    pub(crate) fn from_text(text: &str) -> Memory {
        let mut m = Memory::default();
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else {
                continue;
            };
            let v = v.trim();
            match k {
                "last_ts" => m.last_ts = v.parse().unwrap_or(0),
                "days_seen" => m.days_seen = v.parse().unwrap_or(0),
                "max_streak" => m.max_streak = v.parse().unwrap_or(0),
                "max_secs" => m.max_secs = v.parse().unwrap_or(0),
                "day" => m.day = v.parse().unwrap_or(0),
                "fails" => m.fails = v.parse().unwrap_or(0),
                _ => {
                    if let Some(name) = k.strip_prefix("tf.") {
                        if let Ok(n) = v.parse::<u32>() {
                            m.tool_fails.insert(name.to_string(), n);
                        }
                    } else if let Some(name) = k.strip_prefix("dr.") {
                        if let Ok(n) = v.parse::<u32>() {
                            m.danger_runs.insert(name.to_string(), n);
                        }
                    }
                }
            }
        }
        m
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T0: u64 = 1_000_000_000; // 01:46:40 UTC on its day
    const D: u64 = 86_400;

    fn ob(
        now: u64,
        exit: i32,
        streak: u32,
        tool: Option<Tool>,
        danger: Option<Danger>,
        secs: u64,
    ) -> Obs {
        Obs {
            now,
            offset: 0,
            exit,
            secs,
            streak,
            tool,
            danger,
        }
    }

    fn plain(now: u64) -> Obs {
        ob(now, 0, 0, None, None, 0)
    }

    fn seeded() -> Memory {
        let mut m = Memory::default();
        m.observe(&plain(T0));
        m
    }

    #[test]
    fn first_event_ever_says_hello_once() {
        let mut m = Memory::default();
        assert_eq!(m.observe(&plain(T0)), vec![Fact::FirstEver]);
        assert_eq!(m.days_seen(), 1);
        assert!(m.observe(&plain(T0 + 60)).is_empty());
        assert_eq!(m.days_seen(), 1);
    }

    #[test]
    fn long_gap_in_the_same_day() {
        let mut m = seeded();
        assert_eq!(
            m.observe(&plain(T0 + 5 * 3600)),
            vec![Fact::BackAfterHours { hours: 5 }]
        );
    }

    #[test]
    fn next_day_and_days_away() {
        let mut m = seeded();
        assert_eq!(m.observe(&plain(T0 + D)), vec![Fact::FirstToday]);
        let mut m = seeded();
        assert_eq!(
            m.observe(&plain(T0 + 3 * D)),
            vec![Fact::BackAfterDays { days: 3 }]
        );
    }

    #[test]
    fn day_follows_the_local_offset() {
        // 22:00 local on day N and 02:00 local on day N+1 are 4h apart but on different days
        // in UTC-3 only if the offset is applied.
        let utc_midnight = (T0 / D + 1) * D; // start of the next UTC day
        let mut m = Memory::default();
        let mut o = plain(utc_midnight - 3600); // 23:00 UTC = 20:00 local (UTC-3)
        o.offset = -3 * 3600;
        m.observe(&o);
        let mut o = plain(utc_midnight + 3600); // 01:00 UTC = 22:00 local, same local day
        o.offset = -3 * 3600;
        assert!(m.observe(&o).is_empty());
    }

    #[test]
    fn third_failure_of_a_tool_today() {
        let mut m = seeded();
        for i in 1..=3u32 {
            let f = m.observe(&ob(T0 + 10 * i as u64, 1, i, Some(Tool::Build), None, 0));
            if i < 3 {
                assert!(f.is_empty(), "failure {i}: {f:?}");
            } else {
                assert_eq!(f, vec![Fact::NthFail { tool: Tool::Build, n: 3 }]);
            }
        }
    }

    #[test]
    fn counters_reset_on_a_new_day() {
        let mut m = seeded();
        m.observe(&ob(T0 + 10, 1, 1, Some(Tool::Build), None, 0));
        m.observe(&ob(T0 + 20, 1, 2, Some(Tool::Build), None, 0));
        // Without a reset this would be the 3rd failure and fire NthFail.
        let f = m.observe(&ob(T0 + D, 1, 1, Some(Tool::Build), None, 0));
        assert_eq!(f, vec![Fact::FirstToday]);
        assert!(m.observe(&ob(T0 + D + 10, 1, 2, Some(Tool::Build), None, 0)).is_empty());
    }

    #[test]
    fn dangerous_command_again() {
        let mut m = seeded();
        assert!(m.observe(&ob(T0 + 10, 0, 0, None, Some(Danger::Rm), 0)).is_empty());
        assert_eq!(
            m.observe(&ob(T0 + 20, 0, 0, None, Some(Danger::Rm), 0)),
            vec![Fact::DangerAgain { danger: Danger::Rm, n: 2 }]
        );
    }

    #[test]
    fn streak_record_needs_to_beat_the_old_one() {
        let mut m = seeded();
        assert_eq!(
            m.observe(&ob(T0 + 10, 1, 4, None, None, 0)),
            vec![Fact::StreakRecord { n: 4 }]
        );
        assert!(m.observe(&ob(T0 + 20, 1, 4, None, None, 0)).is_empty());
        assert_eq!(
            m.observe(&ob(T0 + 30, 1, 5, None, None, 0)),
            vec![Fact::StreakRecord { n: 5 }]
        );
    }

    #[test]
    fn slow_record_ignores_interactive_tools() {
        let mut m = seeded();
        assert!(m.observe(&ob(T0 + 10, 0, 0, None, None, 100)).is_empty()); // sets the baseline
        assert_eq!(
            m.observe(&ob(T0 + 20, 0, 0, None, None, 200)),
            vec![Fact::SlowRecord { secs: 200 }]
        );
        assert!(m.observe(&ob(T0 + 30, 0, 0, None, None, 150)).is_empty());
        // two hours in an editor is not a record...
        assert!(m
            .observe(&ob(T0 + 40, 0, 0, Some(Tool::Editor), None, 7200))
            .is_empty());
        // ...and it did not raise the bar either.
        assert_eq!(
            m.observe(&ob(T0 + 50, 0, 0, None, None, 250)),
            vec![Fact::SlowRecord { secs: 250 }]
        );
    }

    #[test]
    fn ctrl_c_is_not_a_failure() {
        let mut m = seeded();
        for i in 0..5u64 {
            assert!(m
                .observe(&ob(T0 + 10 + i, 130, 0, Some(Tool::Build), None, 0))
                .is_empty());
        }
    }

    #[test]
    fn text_roundtrip() {
        let mut m = seeded();
        m.observe(&ob(T0 + 10, 1, 4, Some(Tool::GitPush), Some(Danger::ForcePush), 90));
        m.observe(&ob(T0 + 20, 1, 5, Some(Tool::GitPush), None, 120));
        assert_eq!(Memory::from_text(&m.to_text()), m);
    }

    #[test]
    fn from_text_skips_garbage() {
        let m = Memory::from_text("v=1\nfoo\nlast_ts=abc\nmax_streak=7\n=x\ntf.git-push=2\ndr.rm=zz\n");
        assert_eq!(m.last_ts, 0);
        assert_eq!(m.max_streak, 7);
        assert_eq!(m.tool_fails.get("git-push"), Some(&2));
        assert!(m.danger_runs.is_empty());
        assert_eq!(Memory::from_text(""), Memory::default());
    }

    #[test]
    fn ordinals() {
        assert_eq!(nth(1), "1st");
        assert_eq!(nth(2), "2nd");
        assert_eq!(nth(3), "3rd");
        assert_eq!(nth(4), "4th");
        assert_eq!(nth(11), "11th");
        assert_eq!(nth(21), "21st");
    }
}
