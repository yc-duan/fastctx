//! How a machine or a hub is named (`world-network.md`, 机器的名字).
//!
//! Such a name is letters and digits of any language and hyphens. It is kept
//! as written after NFKC, so full-width forms become half-width and a name
//! comes back the same from any system's decomposition; two names are the
//! same name when their NFKC case folds (NFKC_Casefold) are equal; and a
//! name mixes scripts no further than Unicode's moderately restrictive level
//! (UTS #39, 5.2): Latin goes with Chinese, Japanese or Korean, or with one
//! other script in modern use that is not Cyrillic or Greek. The Unicode
//! data is ICU4X's, so every machine of one release reads a name alike.

use std::borrow::Cow;

use icu_casemap::CaseMapper;
use icu_normalizer::ComposingNormalizerBorrowed;
use icu_properties::props::{DefaultIgnorableCodePoint, GeneralCategory, GeneralCategoryGroup, Script, WhiteSpace};
use icu_properties::script::ScriptWithExtensions;
use icu_properties::{CodePointMapData, CodePointSetData};
use serde::{Deserialize, Serialize};

/// At most this many characters, in the written form and in the key.
pub const MAX_CHARS: usize = 32;

/// At most this many bytes of UTF-8, in the written form and in the key: two
/// names, the dot between them and a timestamp fit in a 255-byte file name.
pub const MAX_BYTES: usize = 64;

/// What is wrong with a name, for the person who typed it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "problem")]
pub enum NameProblem {
    Empty,
    /// More than `MAX_CHARS` characters or `MAX_BYTES` bytes.
    TooLong,
    /// A space, a tab or a line break.
    Space,
    /// Punctuation, a symbol or an invisible character, as typed.
    NotAllowed {
        character: char,
    },
    /// A hyphen or a combining mark first.
    BadStart {
        character: char,
    },
    EndsWithHyphen,
    /// Two characters from scripts that may not be mixed in one name.
    MixedScripts {
        first: char,
        second: char,
    },
    /// A word kept for something else, such as `hub`.
    Reserved,
}

impl std::fmt::Display for NameProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NameProblem::Empty => f.write_str("it is empty"),
            NameProblem::TooLong => write!(f, "it is longer than {MAX_CHARS} characters or {MAX_BYTES} bytes"),
            NameProblem::Space => f.write_str("it has a space"),
            NameProblem::NotAllowed { character } => {
                write!(f, "{character:?} is not a letter, a digit or a hyphen")
            }
            NameProblem::BadStart { character } => write!(f, "it starts with {character:?}"),
            NameProblem::EndsWithHyphen => f.write_str("it ends with a hyphen"),
            NameProblem::MixedScripts { first, second } => {
                write!(f, "{first:?} and {second:?} are from scripts that are not mixed in one name")
            }
            NameProblem::Reserved => f.write_str("the word is reserved"),
        }
    }
}

/// A name that passed the rule: as written, and the key it is compared by.
pub(crate) struct Spelled {
    pub written: String,
    pub key: String,
}

static NFKC: ComposingNormalizerBorrowed<'static> = ComposingNormalizerBorrowed::new_nfkc();

/// `text` in NFKC: full-width forms half-width, compatibility forms plain,
/// composed. What a name is written as, and how a `machine.hub` is read.
pub(crate) fn nfkc(text: &str) -> Cow<'_, str> {
    NFKC.normalize(text)
}

/// Check `text` as a machine or hub name.
pub(crate) fn spell(text: &str) -> Result<Spelled, NameProblem> {
    if text.is_empty() {
        return Err(NameProblem::Empty);
    }
    // Characters are judged as typed, so the problem names what the person
    // wrote, not what normalization made of it.
    for character in text.chars() {
        if CodePointSetData::new::<WhiteSpace>().contains(character) {
            return Err(NameProblem::Space);
        }
        let mut one = [0_u8; 4];
        if is_ignorable(character) || !NFKC.normalize(character.encode_utf8(&mut one)).chars().all(allowed) {
            return Err(NameProblem::NotAllowed { character });
        }
    }
    let written = NFKC.normalize(text).into_owned();
    if let Some(character) = written.chars().find(|&character| !allowed(character)) {
        return Err(NameProblem::NotAllowed { character });
    }
    let first = written.chars().next().expect("NFKC keeps a non-empty name non-empty");
    if first == '-' || GeneralCategoryGroup::Mark.contains(category(first)) {
        return Err(NameProblem::BadStart { character: first });
    }
    if written.ends_with('-') {
        return Err(NameProblem::EndsWithHyphen);
    }
    let key = key_of(&written);
    if too_long(&written) || too_long(&key) {
        return Err(NameProblem::TooLong);
    }
    if let Some((first, second)) = mixed_scripts(&written) {
        return Err(NameProblem::MixedScripts { first, second });
    }
    Ok(Spelled { written, key })
}

/// What a written name is compared by: its NFKC case fold. Two names with
/// the same key are the same name.
fn key_of(written: &str) -> String {
    let folded = CaseMapper::new().fold_string(written);
    NFKC.normalize(&folded).into_owned()
}

fn too_long(text: &str) -> bool {
    text.len() > MAX_BYTES || text.chars().count() > MAX_CHARS
}

fn category(character: char) -> GeneralCategory {
    CodePointMapData::<GeneralCategory>::new().get(character)
}

fn is_ignorable(character: char) -> bool {
    CodePointSetData::new::<DefaultIgnorableCodePoint>().contains(character)
}

/// A letter, a combining mark (no script is written without them), a
/// decimal digit, or a hyphen. Nothing invisible.
fn allowed(character: char) -> bool {
    if character == '-' {
        return true;
    }
    let category = category(character);
    (GeneralCategoryGroup::Letter.contains(category)
        || GeneralCategoryGroup::Mark.contains(category)
        || category == GeneralCategory::DecimalNumber)
        && !is_ignorable(character)
}

/// Windows takes these for devices, in any case and with any extension:
/// every one of them before Windows 11, `nul` still there. Keys are case
/// folded, so lowercase is all there is to match.
fn is_windows_device(key: &str) -> bool {
    match key.as_bytes() {
        b"con" | b"prn" | b"aux" | b"nul" => true,
        [b'c', b'o', b'm', digit] | [b'l', b'p', b't', digit] => digit.is_ascii_digit(),
        _ => false,
    }
}

/// A key as one component of a file name on every system: the key itself,
/// which no case-insensitive or normalizing file system confuses with
/// another key, with `_` after a Windows device name. No key ends with `_`.
pub(crate) fn file_name(key: &str) -> Cow<'_, str> {
    if is_windows_device(key) { Cow::Owned(format!("{key}_")) } else { Cow::Borrowed(key) }
}

/// The scripts of UTS #31's Table 5, "Recommended Scripts" (Unicode 18):
/// the scripts in widespread modern use that Latin may be mixed with,
/// Cyrillic and Greek excepted.
const RECOMMENDED: [Script; 30] = [
    Script::Common,
    Script::Inherited,
    Script::Arabic,
    Script::Armenian,
    Script::Bengali,
    Script::Cyrillic,
    Script::Devanagari,
    Script::Ethiopic,
    Script::Georgian,
    Script::Greek,
    Script::Gujarati,
    Script::Gurmukhi,
    Script::Hangul,
    Script::Han,
    Script::Hebrew,
    Script::Hiragana,
    Script::Katakana,
    Script::Kannada,
    Script::Khmer,
    Script::Lao,
    Script::Latin,
    Script::Malayalam,
    Script::Myanmar,
    Script::Oriya,
    Script::Sinhala,
    Script::Tamil,
    Script::Telugu,
    Script::Thaana,
    Script::Thai,
    Script::Tibetan,
];

/// The scripts a character may belong to, augmented as UTS #39 5.1 says
/// (Han also counts as Han with Bopomofo, Japanese and Korean; kana as
/// Japanese; Hangul as Korean; Bopomofo as Han with Bopomofo). `None` for a
/// Common or Inherited character, which goes with any script.
fn scripts_of(character: char) -> Option<Vec<Script>> {
    let extensions = ScriptWithExtensions::new().get_script_extensions_val(character);
    let mut scripts: Vec<Script> = extensions.iter().collect();
    if scripts.iter().all(|&script| script == Script::Common || script == Script::Inherited) {
        return None;
    }
    let mut augmented = Vec::new();
    for &script in &scripts {
        match script {
            Script::Han => augmented.extend([Script::HanWithBopomofo, Script::Japanese, Script::Korean]),
            Script::Hiragana | Script::Katakana => augmented.push(Script::Japanese),
            Script::Hangul => augmented.push(Script::Korean),
            Script::Bopomofo => augmented.push(Script::HanWithBopomofo),
            _ => {}
        }
    }
    scripts.extend(augmented);
    scripts.sort();
    scripts.dedup();
    Some(scripts)
}

/// The scripts every character of `characters` belongs to; `None` when
/// they are all Common or Inherited.
fn resolved(characters: impl IntoIterator<Item = char>) -> Option<Vec<Script>> {
    let mut resolved: Option<Vec<Script>> = None;
    for scripts in characters.into_iter().filter_map(scripts_of) {
        resolved = Some(match resolved {
            None => scripts,
            Some(mut kept) => {
                kept.retain(|script| scripts.contains(script));
                kept
            }
        });
    }
    resolved
}

/// Whether `characters` stay within UTS #39's moderately restrictive level:
/// one script; or Latin with Han and kana, with Han and Bopomofo, or with
/// Han and Hangul; or Latin with one other recommended script that is not
/// Cyrillic or Greek.
fn restricted_enough(characters: &[char]) -> bool {
    if resolved(characters.iter().copied()).is_none_or(|scripts| !scripts.is_empty()) {
        return true;
    }
    let not_latin = characters
        .iter()
        .copied()
        .filter(|&character| scripts_of(character).is_some_and(|scripts| !scripts.contains(&Script::Latin)));
    let Some(others) = resolved(not_latin) else { return true };
    others.iter().any(|script| {
        matches!(*script, Script::HanWithBopomofo | Script::Japanese | Script::Korean)
            || (RECOMMENDED.contains(script) && *script != Script::Cyrillic && *script != Script::Greek)
    })
}

/// Two characters of `written` that mix scripts, if any: the first one that
/// makes the name too mixed, and an earlier one it clashes with. Case
/// folding keeps every character in its script, so the written form is
/// judged, and the characters named are the ones the person typed.
fn mixed_scripts(written: &str) -> Option<(char, char)> {
    let characters: Vec<char> = written.chars().collect();
    if restricted_enough(&characters) {
        return None;
    }
    let last = (1..characters.len()).find(|&end| !restricted_enough(&characters[..=end]))?;
    let second = characters[last];
    let first = characters[..last]
        .iter()
        .copied()
        .find(|&earlier| !restricted_enough(&[earlier, second]))
        .or_else(|| characters[..last].iter().copied().find(|&earlier| scripts_of(earlier).is_some()))
        .unwrap_or(characters[0]);
    Some((first, second))
}

#[cfg(test)]
mod tests;
