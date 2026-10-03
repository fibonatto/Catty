//! The cat's built-in voice: pure data keyed by moment. No I/O, no state.

use crate::util::{human_secs, strs};
use crate::Moment;

pub(crate) fn danger_desc(k: &str) -> &'static str {
    match k {
        "rm" => "recursive forced delete",
        "force-push" => "git push --force",
        "reset-hard" => "git reset --hard",
        "dd" => "dd writing onto a device",
        "mkfs" => "formatting a disk",
        _ => "something risky",
    }
}

fn tool_lines(key: &str) -> Vec<String> {
    match key {
        "git-commit" => strs(&[
            "committed. i'm proud. a little.",
            "another snapshot of your questionable decisions.",
            "history, written. mrrp.",
        ]),
        "git-push" => strs(&["it's out there now. no take-backs.", "off it goes. mew."]),
        "git-pull" => strs(&[
            "fresh things from far away.",
            "what did the others break today?",
        ]),
        "git-stash" => strs(&["hiding things? i respect it.", "into the drawer it goes."]),
        "git-branch" => strs(&[
            "new branch. new hopes.",
            "*follows you to the other branch*",
        ]),
        "git-merge" => strs(&[
            "bringing the strays together.",
            "merge conflicts are just cats fighting.",
        ]),
        "git" => strs(&["git again. fascinating.", "history lessons, hm."]),
        "editor" => strs(&[
            "back from the editor cave.",
            "did you write anything, or just stare?",
        ]),
        "ssh" => strs(&[
            "welcome back. did you bring fish?",
            "other machines smell different, don't they.",
        ]),
        "cat" => strs(&["that's my name, you know.", "*offended purr*"]),
        "sleep" => strs(&["nap time. finally, someone gets it.", "i'm better at that."]),
        "clear" => strs(&["a clean slate. i hid the evidence.", "tidy. suspicious."]),
        "man" => strs(&[
            "reading the manual? who are you.",
            "*is impressed, against own will*",
        ]),
        "top" => strs(&["watching the numbers dance.", "so many little processes."]),
        "build" => strs(&[
            "the machine hums. i approve.",
            "building things again. good.",
        ]),
        "container" => strs(&["boxes inside boxes. i approve.", "i do love a good box."]),
        "ping" => strs(&["is anybody out there? mew.", "knock knock."]),
        "net" => strs(&[
            "fetching things from far away.",
            "*ears twitch at the packets*",
        ]),
        "ollama" => strs(&["ah. a rival.", "is that my cousin? i can't tell."]),
        "search" => strs(&["hunting? i approve.", "*tail swish* what are we stalking?"]),
        "rm" => strs(&["gone. just like that.", "a clean kill. mrrp."]),
        "sudo" => strs(&["ooh, powers.", "with great power comes... a nap."]),
        _ => strs(&["mrrp."]),
    }
}

pub(crate) fn bank(m: &Moment) -> Vec<String> {
    match m {
        Moment::Fail {
            code,
            streak,
            repeat,
        } => {
            if *streak >= 3 {
                vec![
                    format!("{streak} in a row. i'm counting."),
                    format!("...{streak}. want me to look away?"),
                    "we're in a loop. i can feel it.".to_string(),
                ]
            } else if *repeat {
                strs(&[
                    "same command. same result.",
                    "again? bold strategy.",
                    "i'd try something different. just saying.",
                    "the definition of insanity, you know.",
                ])
            } else if *code == 139 {
                strs(&["segfault. classic.", "memory is a dangerous place."])
            } else if *code == 137 || *code == 143 {
                strs(&[
                    "something got killed. rudely.",
                    "it didn't even say goodbye.",
                ])
            } else {
                strs(&[
                    "well. that didn't work.",
                    "mew. that sounded painful.",
                    "*watches the error scroll by*",
                    "it said no.",
                    "brave. wrong, but brave.",
                    "i saw nothing. nothing at all.",
                ])
            }
        }
        Moment::NotFound => strs(&[
            "that's not a command. that's a wish.",
            "typo? or optimism.",
            "no such thing. i checked.",
            "the shell doesn't know that word either.",
        ]),
        Moment::Interrupted => strs(&[
            "you gave up. valid.",
            "ctrl-c. the quitter's choice. i respect it.",
            "stopped it midair. wise or impatient?",
        ]),
        Moment::Slow { secs } => {
            let h = human_secs(*secs);
            vec![
                format!("{h} to finish. i napped through it."),
                format!("that took {h}. i dreamed of fish."),
                "finally. my whiskers aged.".to_string(),
            ]
        }
        Moment::Danger(k) => match *k {
            "rm" => strs(&[
                "*stares at the rm -rf*",
                "my fur is standing up.",
                "i hope you meant that.",
            ]),
            "force-push" => strs(&[
                "force push. bold. i'm hiding.",
                "rewriting history, are we.",
            ]),
            "reset-hard" => strs(&["hard reset. goodbye, work.", "...it's gone, isn't it."]),
            "dd" => strs(&[
                "dd. the disk destroyer. careful.",
                "i'm not saying don't. i'm saying mew.",
            ]),
            "mkfs" => strs(&[
                "a fresh disk. such confidence.",
                "everything on it was a dream now.",
            ]),
            _ => strs(&["that looked risky. mew."]),
        },
        Moment::Tool(k) => tool_lines(k),
        Moment::Plain => strs(&[
            "mrrp.",
            "hm.",
            "*blinks slowly*",
            "i'm watching.",
            "carry on.",
            "interesting.",
            "*tail flick*",
            "prrr.",
        ]),
    }
}
