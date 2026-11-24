//! What a caller is told of the callees it is offered: under each one's
//! name, when to ask it and what its preset does; after them all, the texts
//! on how to ask, each once, under the names it is for where they differ.

use fastctx_types::collab::Callee;

/// When to ask `callee`, as its account wrote it, then what its preset does.
pub(super) fn line(callee: &Callee) -> String {
    match (callee.use_when.trim(), callee.kind().does()) {
        ("", does) => does.to_string(),
        (when, "") => when.to_string(),
        (when, does) => format!("{when}\n{does}"),
    }
}

/// The texts for callers of the callees offered, in the order they are.
#[derive(Default)]
pub(super) struct Texts {
    told: Vec<(String, Vec<String>)>,
    callees: usize,
}

impl Texts {
    /// `text` is for the callee offered as `name`; a preset with none of
    /// its own says what it does under its name alone.
    pub(super) fn add(&mut self, name: &str, text: &str) {
        self.callees += 1;
        if text.trim().is_empty() {
            return;
        }
        match self.told.iter_mut().find(|(told, _)| told == text) {
            Some((_, names)) => names.push(name.to_string()),
            None => self.told.push((text.to_string(), vec![name.to_string()])),
        }
    }

    /// One text every callee shares stands alone, as it did before presets;
    /// otherwise each stands under the names of the callees it is for.
    pub(super) fn told(self) -> String {
        match self.told.as_slice() {
            [(text, names)] if names.len() == self.callees => text.clone(),
            told => {
                told.iter().map(|(text, names)| format!("{}:\n{text}", names.join(", "))).collect::<Vec<_>>().join("\n\n")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fastctx_types::collab::AgentLogin;
    use fastctx_types::collab_presets::{Kind, Preset, REVIEW_DOES};

    fn callee(name: &str, use_when: &str, preset: Option<Preset>) -> Callee {
        Callee {
            name: name.into(),
            use_when: use_when.into(),
            login: AgentLogin { machine: "here".parse().unwrap(), agent: "codex".parse().unwrap(), home: None },
            model: None,
            effort: None,
            screen: true,
            at_once: None,
            preset,
        }
    }

    #[test]
    fn a_callee_from_before_presets_is_said_as_the_reviewer() {
        assert_eq!(line(&callee("gpt", "", None)), REVIEW_DOES);
        assert_eq!(line(&callee("gpt", " the hard ones ", None)), format!("the hard ones\n{REVIEW_DOES}"));
        let own = Preset { kind: Kind::Own, prompt: None, rules: String::new() };
        assert_eq!(line(&callee("mine", "when I say", Some(own))), "when I say");
    }

    #[test]
    fn one_text_shared_stands_alone_and_differing_ones_under_their_names() {
        let mut texts = Texts::default();
        texts.add("gpt", "how to ask");
        texts.add("gpt-2", "how to ask");
        assert_eq!(texts.told(), "how to ask");

        let mut texts = Texts::default();
        texts.add("gpt", "how to ask");
        texts.add("scout", "how to send");
        texts.add("gpt-2", "how to ask");
        texts.add("mine", "");
        assert_eq!(texts.told(), "gpt, gpt-2:\nhow to ask\n\nscout:\nhow to send");

        let mut texts = Texts::default();
        texts.add("gpt", "how to ask");
        texts.add("mine", " ");
        assert_eq!(texts.told(), "gpt:\nhow to ask");
    }
}
