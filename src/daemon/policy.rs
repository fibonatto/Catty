//! When does the cat speak? Pure decision logic: no I/O, no locks.

use crate::moment::{base_chance, is_strong, Moment};
use crate::util::Rng;

/// Prompts that must pass since the cat last spoke in a shell, unless the moment is strong.
const MIN_GAP: u32 = 2;

/// Odds (before CATTY_CHATTINESS) when the memory has something to say.
const NOTABLE_CHANCE: u32 = 85;

/// `notable`: the memory produced a fact for this command (a record, a return, a milestone).
pub(crate) fn may_speak(moment: &Moment, gap: u32, notable: bool) -> bool {
    notable || is_strong(moment) || gap >= MIN_GAP
}

/// Odds (0..=100) that the cat speaks, scaled by CATTY_CHATTINESS (100 = normal).
pub(crate) fn speak_chance(moment: &Moment, chattiness: u32, notable: bool) -> u32 {
    let base = base_chance(moment);
    let base = if notable {
        base.max(NOTABLE_CHANCE)
    } else {
        base
    };
    (base * chattiness / 100).min(100)
}

pub(crate) fn should_speak(
    moment: &Moment,
    gap: u32,
    chattiness: u32,
    notable: bool,
    rng: &mut Rng,
) -> bool {
    may_speak(moment, gap, notable) && rng.chance(speak_chance(moment, chattiness, notable))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::{Danger, Tool};

    fn fail(streak: u32) -> Moment {
        Moment::Fail {
            code: 1,
            streak,
            repeat: false,
        }
    }

    #[test]
    fn gap_gate() {
        assert!(!may_speak(&Moment::Plain, MIN_GAP - 1, false));
        assert!(may_speak(&Moment::Plain, MIN_GAP, false));
        assert!(!may_speak(&Moment::Tool(Tool::Git), 0, false));
    }

    #[test]
    fn strong_moments_ignore_the_gap() {
        assert!(may_speak(&Moment::Danger(Danger::Rm), 0, false));
        assert!(may_speak(&fail(3), 0, false));
        assert!(!may_speak(&fail(2), 0, false));
        assert!(may_speak(&Moment::Slow { secs: 120 }, 0, false));
        assert!(!may_speak(&Moment::Slow { secs: 119 }, 0, false));
    }

    #[test]
    fn notable_facts_ignore_the_gap_and_raise_the_odds() {
        assert!(may_speak(&Moment::Plain, 0, true));
        assert_eq!(speak_chance(&Moment::Plain, 100, true), NOTABLE_CHANCE);
        assert_eq!(speak_chance(&Moment::Plain, 0, true), 0); // chattiness 0 still silences it
        assert_eq!(speak_chance(&Moment::Danger(Danger::Rm), 100, true), 95); // never lowered
    }

    #[test]
    fn chance_scales_with_chattiness_and_caps_at_100() {
        assert_eq!(speak_chance(&Moment::Plain, 0, false), 0);
        assert_eq!(speak_chance(&Moment::Plain, 100, false), 8);
        assert_eq!(speak_chance(&Moment::Plain, 200, false), 16);
        assert_eq!(speak_chance(&Moment::Danger(Danger::Rm), 200, false), 100);
        assert_eq!(speak_chance(&Moment::Slow { secs: 300 }, 100, false), 100);
    }

    // At 0% and at 100% the dice are irrelevant, so these are deterministic.
    #[test]
    fn extremes_do_not_depend_on_the_dice() {
        let mut rng = Rng::new();
        let danger = Moment::Danger(Danger::Rm);
        for _ in 0..200 {
            assert!(!should_speak(&danger, 0, 0, false, &mut rng)); // silent
            assert!(should_speak(&danger, 0, 200, false, &mut rng)); // capped at 100
            assert!(!should_speak(&Moment::Plain, 0, 200, false, &mut rng)); // gap gate closed
            assert!(should_speak(&Moment::Plain, 0, 200, true, &mut rng)); // notable: 170 -> 100
            assert!(!should_speak(&Moment::Plain, 0, 0, true, &mut rng)); // notable but silenced
        }
    }
}
