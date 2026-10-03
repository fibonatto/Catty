//! The cat's built-in voice: pure data keyed by moment. No I/O, no state.

use crate::command::{Danger, Tool};
use crate::moment::Moment;
use crate::util::{human_secs, strs};

pub(crate) fn danger_desc(d: Danger) -> &'static str {
    match d {
        Danger::Rm => "recursive forced delete",
        Danger::ForcePush => "git push --force",
        Danger::ResetHard => "git reset --hard",
        Danger::Dd => "dd writing onto a device",
        Danger::Mkfs => "formatting a disk",
    }
}

fn tool_lines(tool: Tool) -> Vec<String> {
    match tool {
        Tool::GitCommit => strs(&[
            "committed. i'm proud. a little.",
            "another snapshot of your questionable decisions.",
            "history, written. mrrp.",
        ]),
        Tool::GitPush => strs(&["it's out there now. no take-backs.", "off it goes. mew."]),
        Tool::GitPull => strs(&[
            "fresh things from far away.",
            "what did the others break today?",
        ]),
        Tool::GitStash => strs(&["hiding things? i respect it.", "into the drawer it goes."]),
        Tool::GitBranch => strs(&[
            "new branch. new hopes.",
            "*follows you to the other branch*",
        ]),
        Tool::GitMerge => strs(&[
            "bringing the strays together.",
            "merge conflicts are just cats fighting.",
        ]),
        Tool::Git => strs(&["git again. fascinating.", "history lessons, hm."]),
        Tool::Editor => strs(&[
            "back from the editor cave.",
            "did you write anything, or just stare?",
        ]),
        Tool::Ssh => strs(&[
            "welcome back. did you bring fish?",
            "other machines smell different, don't they.",
        ]),
        Tool::Cat => strs(&["that's my name, you know.", "*offended purr*"]),
        Tool::Sleep => strs(&["nap time. finally, someone gets it.", "i'm better at that."]),
        Tool::Clear => strs(&["a clean slate. i hid the evidence.", "tidy. suspicious."]),
        Tool::Man => strs(&[
            "reading the manual? who are you.",
            "*is impressed, against own will*",
        ]),
        Tool::Top => strs(&["watching the numbers dance.", "so many little processes."]),
        Tool::Build => strs(&[
            "the machine hums. i approve.",
            "building things again. good.",
        ]),
        Tool::Container => strs(&["boxes inside boxes. i approve.", "i do love a good box."]),
        Tool::Ping => strs(&["is anybody out there? mew.", "knock knock."]),
        Tool::Net => strs(&[
            "fetching things from far away.",
            "*ears twitch at the packets*",
        ]),
        Tool::Ollama => strs(&["ah. a rival.", "is that my cousin? i can't tell."]),
        Tool::Search => strs(&["hunting? i approve.", "*tail swish* what are we stalking?"]),
        Tool::Rm => strs(&["gone. just like that.", "a clean kill. mrrp."]),
        Tool::Sudo => strs(&["ooh, powers.", "with great power comes... a nap."]),
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
            Danger::Rm => strs(&[
                "*stares at the rm -rf*",
                "my fur is standing up.",
                "i hope you meant that.",
            ]),
            Danger::ForcePush => strs(&[
                "force push. bold. i'm hiding.",
                "rewriting history, are we.",
            ]),
            Danger::ResetHard => strs(&["hard reset. goodbye, work.", "...it's gone, isn't it."]),
            Danger::Dd => strs(&[
                "dd. the disk destroyer. careful.",
                "i'm not saying don't. i'm saying mew.",
            ]),
            Danger::Mkfs => strs(&[
                "a fresh disk. such confidence.",
                "everything on it was a dream now.",
            ]),
        },
        Moment::Tool(k) => tool_lines(*k),
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
