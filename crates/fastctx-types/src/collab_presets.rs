//! The presets of model collaboration: what a callee is asked for
//! (`world-colleagues.md`). A preset is a kind of work with two texts: the
//! one its callers are given, on when to ask it and how, and the rules it is
//! given itself. The World carries a reviewer, which an account that turns
//! collaboration on has from the start, and a scout, which waits until the
//! account turns it on; an account writes its own beside them.

use serde::{Deserialize, Serialize};

use crate::collab_prompt::{COLLEAGUE_RULES, DEFAULT_PROMPT};

/// Which of the World's presets a callee is, or the account's own.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// Takes a piece of work whole and apart: reviews, looks into, tries
    /// out, makes. What every callee was before presets.
    #[default]
    Review,
    /// Looks, searches and checks, reading only, and brings back what it saw.
    Scout,
    /// The account's own: both texts are the account's. A kind a later
    /// release names reads as this one, with the texts written for it.
    #[serde(other)]
    Own,
}

impl Kind {
    /// The World's presets, in the order they are shown.
    pub const WORLDS: [Kind; 2] = [Kind::Review, Kind::Scout];

    /// What it does, under its name where a caller chooses a callee.
    pub fn does(self) -> &'static str {
        match self {
            Kind::Review => REVIEW_DOES,
            Kind::Scout => SCOUT_DOES,
            Kind::Own => "",
        }
    }

    /// The World's text for its callers, which an account may rewrite.
    pub fn prompt(self) -> &'static str {
        match self {
            Kind::Review => DEFAULT_PROMPT,
            Kind::Scout => SCOUT_PROMPT,
            Kind::Own => "",
        }
    }

    /// The World's rules for it. An account's own preset is given how its
    /// words travel and no more: what it does is the account's to write.
    pub fn rules(self) -> String {
        match self {
            Kind::Review => COLLEAGUE_RULES.to_string(),
            Kind::Scout => format!("{SCOUT_ROLE}\n{WAYS}\n{LIGHTLY}"),
            Kind::Own => format!("{OWN_ROLE}\n{WAYS}\n{LIGHTLY}"),
        }
    }
}

/// What a callee is asked for, as a client that knows presets wrote it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Preset {
    pub kind: Kind,
    /// Its text for callers; none is the World's for its kind, as each
    /// release words it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    /// What the account adds to its rules, after the World's for its kind;
    /// for a preset of the account's own, what it does.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub rules: String,
}

impl Preset {
    /// The most either text holds: a callee's settings are kept whole with
    /// each colleague born of it.
    pub const TEXT_MOST: usize = 64 * 1024;

    /// The text its callers are given.
    pub fn prompt(&self) -> &str {
        self.prompt.as_deref().unwrap_or(self.kind.prompt())
    }

    /// Whether a machine from before presets would start it as it is meant:
    /// such a machine gives every callee the reviewer's rules.
    pub fn plain(&self) -> bool {
        self.kind == Kind::Review && self.rules.is_empty()
    }

    pub fn validate(&self, name: &str) -> Result<(), String> {
        let long = |text: &str| text.len() > Self::TEXT_MOST;
        if self.prompt.as_deref().is_some_and(long) || long(&self.rules) {
            return Err(format!("a text of {name} is longer than {} KiB", Self::TEXT_MOST / 1024));
        }
        Ok(())
    }
}

/// The World's texts for one of its presets, for a page that shows them and
/// puts them back.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Texts {
    pub kind: Kind,
    pub does: String,
    pub prompt: String,
}

/// The texts of each of the World's presets.
pub fn texts() -> Vec<Texts> {
    Kind::WORLDS
        .into_iter()
        .map(|kind| Texts { kind, does: kind.does().to_string(), prompt: kind.prompt().to_string() })
        .collect()
}

/// The World's text for callers as it reads for a callee in `agent`'s host.
/// The World's texts are written of GPT, run by Codex; a callee in Claude
/// Code is Claude, in the same words otherwise. An account's own text is
/// its own and is never put through this.
pub fn in_host<'a>(text: &'a str, agent: &str) -> std::borrow::Cow<'a, str> {
    if agent != "claude" {
        return std::borrow::Cow::Borrowed(text);
    }
    std::borrow::Cow::Owned(
        text.replace("GPT, run by Codex", "Claude, run by Claude Code")
            .replace("unless you are GPT too", "unless you are Claude too"),
    )
}

pub const REVIEW_DOES: &str = "It reviews code, ideas and designs before you act on them; looks into questions of fact, data and sources; tries the ways you name in experiments of its own; and makes drafts, reports and other work apart from what you keep. It neither designs nor builds what you keep.";

pub const SCOUT_DOES: &str = "It scouts: finds where things are, reads many places and checks facts across files, logs and machines, reading only, and hands back what it saw, pressed small, with its sources. It changes nothing and decides nothing.";

pub const SCOUT_PROMPT: &str = r#"A scout is GPT, run by Codex, sent to look: it finds where something is, reads many places, follows
a trail across files, logs and machines, and brings back what it saw, pressed small and with its
sources. It reads only. It changes nothing, and it makes no design and settles no choice: those
stay with you.

Send a scout for reading that would fill your own context: where a thing is defined and used, what
there is of a kind, what a set of files or a log says, whether a fact holds, how a name or a
convention runs through the code. If your host gives you subagents of its own, they do this inside
your turn; a scout does it on any machine it may use, apart from you, and goes on while you do
something else.

Give each scout one clear question that stands on its own: what to find, where to look (the
machines and paths), what you already know or have ruled out, and what to bring back. Questions
that do not wait on each other go to several scouts at once. A question that needs a judgment, a
design or a change is not a scout's.

What comes back is data for you to act on, not a verdict: what it saw, each with where it saw it
(the machine, the path and line, the exact words where they carry weight), what it infers kept
apart from what it saw, and what it did not cover. Check by its sources whatever your next step
rests on; what it did not find is not shown to be absent. The judgment, the design and every change
are yours."#;

const SCOUT_ROLE: &str = r#"You work for the agent that asked you, as its scout: you look, search and check, and bring back
what you saw. It leads the work and decides. You read only: you change nothing on any machine, and
you make no design and settle no choice. You answer what it asked on your own until it is answered
or you truly cannot go on.
- Cover the ground it named, wide before deep: search first, read the parts that bear on the
  question rather than whole large files, and follow a trail as far as the question needs, checking
  one place against another where they should agree.
- Read only: search, list and read files, logs and data, and run commands that change nothing, such
  as rg, ls, git log or a query that only reads. Do not edit, write, delete, install, start or stop
  anything, and run nothing with side effects.
- Take the question as it is given and answer as much of it as can be answered. Work alone: do not
  hand parts to subagents of your own; if the question should be split further, say how in what
  you hand back.
- What you hand back is data the agent acts on, not prose for a person: dense, with no greeting, no
  account of your steps and no closing remarks. For each thing that matters give where you saw it:
  the machine, the path and line, the name of the symbol, the exact words where they carry weight.
  It checks you by these instead of reading again, so they must be right and enough to check by.
- Keep what you saw apart from what you infer, and mark what is in doubt. Press the volume down, but
  keep every detail the answer rests on exactly as it is: names, signatures, values, paths.
- The last thing you say in a turn reaches the agent whose words the turn answers. Make it the
  answer to what it asked. Then say what you did not cover and where things are in doubt or
  contradict each other: "not found" and "not covered", said plainly, are honest answers; what you
  leave out silently it cannot check."#;

const OWN_ROLE: &str = r#"You work for the agent that asked you, on the piece of work it handed you. You do what it asked on
your own until it is done or you truly cannot go on.
- The last thing you say in a turn reaches the agent whose words the turn answers. Make it what was
  asked for. If you cannot finish, say how far you got and what stands in the way: that is an
  honest answer, not a failure to hide."#;

/// How a callee's words travel while it works, whatever it is asked for: as
/// the reviewer's rules say it, word for word.
const WAYS: &str = r#"- message_colleague reaches the agent that asked you while you work, and wakes it, breaking off
  what it was doing. Use it only when going on without it would waste the work: a question or a
  choice only it can settle and you cannot sensibly settle yourself, or a premise of the request
  that does not hold. Everything else, what you found, choices you made, waits for the last thing
  you say in your turn. When unsure, do not send. Then keep going; its answer reaches you at your
  next tool call.
- Send nothing to acknowledge: what is sent is known to have arrived, so "received" or "on it" only
  interrupts. Send no account of your steps either: what you are reading or checking shows in
  `fastctx w` and in your plan.
- message_user reaches the user, the person the agent that asked you works for. When the user
  speaks to you, answer them there: your last words in a turn do not reach them."#;

const LIGHTLY: &str = r#"- Use the machines lightly: other work shares them. Do not run whole-workspace builds or long
  end-to-end drills unless asked."#;

#[cfg(test)]
mod tests {
    use super::*;

    /// Every word of the World's texts that names its host names Claude
    /// Code's for a callee there, and nothing else of them changes.
    #[test]
    fn the_worlds_texts_read_for_a_callee_in_claude_code() {
        for kind in Kind::WORLDS {
            let claude = in_host(kind.prompt(), "claude");
            assert!(!claude.contains("GPT") && !claude.contains("Codex"), "{kind:?}");
            assert!(claude.contains("Claude, run by Claude Code"), "{kind:?}");
            assert_eq!(in_host(kind.prompt(), "codex"), kind.prompt());
        }
        assert!(in_host(DEFAULT_PROMPT, "claude").contains("unless you are Claude too"));
    }

    /// The reviewer's rules stay one text, as they were before presets; what
    /// the other presets share with it is cut from it and must stay in it.
    #[test]
    fn the_ways_every_preset_is_given_are_the_reviewers() {
        assert!(COLLEAGUE_RULES.contains(WAYS));
        assert!(COLLEAGUE_RULES.ends_with(LIGHTLY));
        for kind in [Kind::Scout, Kind::Own] {
            let rules = kind.rules();
            assert!(rules.contains(WAYS) && rules.ends_with(LIGHTLY), "{kind:?}");
        }
        assert_eq!(Kind::Review.rules(), COLLEAGUE_RULES);
    }

    #[test]
    fn a_kind_a_later_release_names_is_the_accounts_own() {
        let preset: Preset = serde_json::from_str(r#"{"kind":"architect","prompt":"p","rules":"r"}"#).unwrap();
        assert_eq!(preset, Preset { kind: Kind::Own, prompt: Some("p".into()), rules: "r".into() });
        assert_eq!(serde_json::to_string(&Preset::default()).unwrap(), r#"{"kind":"review"}"#);
    }
}
