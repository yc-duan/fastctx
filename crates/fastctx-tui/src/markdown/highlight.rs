//! Colours for the code in a fenced block, read off the grammar of its
//! language. Scopes become the roles a page already paints with, so code
//! takes each theme's own colours and degrades with the terminal's depth
//! like everything else.
//!
//! A message being written is laid out again each time it grows. Its blocks
//! are remembered line by line with the grammar's state after each, so only
//! the lines that changed are read again.

use std::sync::OnceLock;

use syntect::parsing::{ParseState, Scope, ScopeStack, SyntaxSet};

use crate::design::Role;

/// A run of code and the role it is painted in.
pub type Piece = (Role, String);

/// The code blocks of one text, by their order in it, as last coloured.
#[derive(Default)]
pub struct Memo {
    blocks: Vec<Option<Remembered>>,
}

struct Remembered {
    language: String,
    lines: Vec<String>,
    /// The grammar's state after each line.
    after: Vec<(ParseState, ScopeStack)>,
    pieces: Vec<Vec<Piece>>,
}

impl Memo {
    /// Each line of the `block`th code block, `code`, without its line
    /// break, as pieces; `None` when no grammar knows `language` or the
    /// grammar fails on the code.
    pub fn highlight(&mut self, block: usize, language: &str, code: &str) -> Option<Vec<Vec<Piece>>> {
        let syntaxes = syntaxes();
        let syntax = syntaxes.find_syntax_by_token(language)?;
        if self.blocks.len() <= block {
            self.blocks.resize_with(block + 1, || None);
        }
        let lines: Vec<&str> = code.split('\n').collect();
        let previous = self.blocks[block].take().filter(|previous| previous.language == language);
        let same = previous
            .as_ref()
            .map_or(0, |previous| previous.lines.iter().zip(&lines).take_while(|(before, now)| before == now).count());
        let mut remembered = match previous {
            Some(mut previous) if same > 0 => {
                previous.lines.truncate(same);
                previous.after.truncate(same);
                previous.pieces.truncate(same);
                previous
            }
            _ => {
                Remembered { language: language.to_string(), lines: Vec::new(), after: Vec::new(), pieces: Vec::new() }
            }
        };
        let (mut state, mut stack) = match remembered.after.last() {
            Some((state, stack)) => (state.clone(), stack.clone()),
            None => (ParseState::new(syntax), ScopeStack::new()),
        };
        for text in &lines[same..] {
            // Grammars are written for lines that end in a line break; the
            // break is read but never shown, whichever scope it closes.
            let line = format!("{text}\n");
            let shown = text.len();
            let mut pieces: Vec<Piece> = Vec::new();
            let mut at = 0;
            for (offset, op) in state.parse_line(&line, syntaxes).ok()? {
                push(&mut pieces, role(&stack), &line[at.min(shown)..offset.min(shown)]);
                at = offset;
                stack.apply(&op).ok()?;
            }
            push(&mut pieces, role(&stack), &line[at.min(shown)..shown]);
            remembered.lines.push(text.to_string());
            remembered.after.push((state.clone(), stack.clone()));
            remembered.pieces.push(pieces);
        }
        let pieces = remembered.pieces.clone();
        self.blocks[block] = Some(remembered);
        Some(pieces)
    }

    /// Forgets the blocks past the first `count`, which the text no longer has.
    pub fn keep(&mut self, count: usize) {
        self.blocks.truncate(count);
    }
}

fn push(pieces: &mut Vec<Piece>, role: Role, text: &str) {
    if text.is_empty() {
        return;
    }
    match pieces.last_mut() {
        Some((last, run)) if *last == role => run.push_str(text),
        _ => pieces.push((role, text.to_string())),
    }
}

/// The role of the innermost scope a rule names; plain text otherwise.
fn role(stack: &ScopeStack) -> Role {
    let rules = rules();
    for scope in stack.as_slice().iter().rev() {
        if let Some((_, role)) = rules.iter().find(|(prefix, _)| prefix.is_prefix_of(*scope)) {
            return *role;
        }
    }
    Role::Text
}

/// Which scopes take which role, the more particular first: the few colours
/// a reader tells apart at a glance, and quiet everywhere else.
fn rules() -> &'static [(Scope, Role)] {
    static RULES: OnceLock<Vec<(Scope, Role)>> = OnceLock::new();
    RULES.get_or_init(|| {
        [
            ("comment", Role::Faint),
            ("punctuation.definition.comment", Role::Faint),
            ("markup.inserted", Role::Alive),
            ("markup.deleted", Role::Wrong),
            ("meta.diff.range", Role::Link),
            ("meta.diff.header", Role::Muted),
            ("string", Role::Alive),
            ("constant.character.escape", Role::Caution),
            ("constant", Role::Caution),
            ("keyword.operator", Role::Text),
            ("keyword", Role::Accent),
            ("storage", Role::Accent),
            ("entity.name.function", Role::Link),
            ("support.function", Role::Link),
            ("entity.name", Role::Link),
            ("support.type", Role::Link),
            ("support.class", Role::Link),
            ("entity.other.attribute-name", Role::Link),
            ("variable.function", Role::Link),
        ]
        .into_iter()
        .map(|(name, role)| (Scope::new(name).expect("a valid scope"), role))
        .collect()
    })
}

fn syntaxes() -> &'static SyntaxSet {
    static SYNTAXES: OnceLock<SyntaxSet> = OnceLock::new();
    SYNTAXES.get_or_init(two_face::syntax::extra_newlines)
}
