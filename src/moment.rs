//! What just happened at the prompt (`Moment`), and how likely the cat is to react.
//! Pure functions, no I/O and no randomness.

use crate::command::{Danger, Tool};

#[derive(Debug, PartialEq)]
pub(crate) enum Moment {
    Fail {
        code: i32,
        streak: u32,
        repeat: bool,
    },
    NotFound,
    Interrupted,
    Slow {
        secs: u64,
    },
    Danger(Danger),
    Tool(Tool),
    Plain,
}

pub(crate) fn classify(
    exit: i32,
    secs: u64,
    streak: u32,
    repeat: bool,
    tool: Option<Tool>,
    danger: Option<Danger>,
) -> Moment {
    if let Some(d) = danger {
        return Moment::Danger(d);
    }
    match exit {
        130 => Moment::Interrupted,
        127 => Moment::NotFound,
        0 => {
            let interactive = tool.is_some_and(Tool::is_interactive);
            if secs >= 20 && !interactive {
                Moment::Slow { secs }
            } else if let Some(t) = tool {
                Moment::Tool(t)
            } else {
                Moment::Plain
            }
        }
        code => Moment::Fail {
            code,
            streak,
            repeat,
        },
    }
}

pub(crate) fn base_chance(m: &Moment) -> u32 {
    match m {
        Moment::Fail { streak, repeat, .. } => {
            if *streak >= 3 {
                90
            } else if *repeat {
                75
            } else {
                55
            }
        }
        Moment::NotFound => 70,
        Moment::Interrupted => 30,
        Moment::Slow { secs } => {
            if *secs >= 300 {
                100
            } else if *secs >= 60 {
                90
            } else {
                70
            }
        }
        Moment::Danger(_) => 95,
        Moment::Tool(_) => 35,
        Moment::Plain => 8,
    }
}

// Strong moments may speak even inside the cooldown window.
pub(crate) fn is_strong(m: &Moment) -> bool {
    matches!(m, Moment::Danger(_))
        || matches!(m, Moment::Fail { streak, .. } if *streak >= 3)
        || matches!(m, Moment::Slow { secs } if *secs >= 120)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classification() {
        assert_eq!(classify(127, 0, 1, false, None, None), Moment::NotFound);
        assert_eq!(classify(130, 0, 0, false, None, None), Moment::Interrupted);
        assert_eq!(
            classify(0, 45, 0, false, Some(Tool::Build), None),
            Moment::Slow { secs: 45 }
        );
        assert_eq!(
            classify(0, 900, 0, false, Some(Tool::Editor), None),
            Moment::Tool(Tool::Editor)
        );
        assert_eq!(
            classify(0, 1, 0, false, None, Some(Danger::Rm)),
            Moment::Danger(Danger::Rm)
        );
    }
}
