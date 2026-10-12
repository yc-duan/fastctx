//! The rule is a frozen contract: a name's key is its identity on every
//! machine and in every file kept under it, so it may never change for a
//! name already in use.

use icu_casemap::CaseMapper;
use icu_properties::CodePointSetData;
use icu_properties::props::{ChangesWhenNfkcCasefolded, DefaultIgnorableCodePoint};

use super::{NameProblem, file_name, key_of, spell};
use crate::MachineName;

/// The key is NFKC_Casefold: for every code point names may hold (Default
/// Ignorables never are), it changes exactly the ones Unicode says it
/// changes, and is stable once applied.
#[test]
fn key_is_nfkc_casefold() {
    let changes = CodePointSetData::new::<ChangesWhenNfkcCasefolded>();
    let ignorable = CodePointSetData::new::<DefaultIgnorableCodePoint>();
    for character in (0..=0x10FFFF_u32).filter_map(char::from_u32).filter(|&character| !ignorable.contains(character)) {
        let one = character.to_string();
        let key = key_of(&super::nfkc(&one));
        assert_eq!(key != one, changes.contains(character), "U+{:04X}", character as u32);
        assert_eq!(key_of(&key), key, "U+{:04X}", character as u32);
    }
    assert_eq!(CaseMapper::new().fold_string("ẞ"), "ss");
}

/// Every name the earlier rule took is still itself: as written, as its
/// key, and as a file name, except a Windows device name.
#[test]
fn earlier_names_are_unchanged() {
    for name in ["laptop", "gpu-2", "desktop", "a", "x1-y2-z3", "abcdefghijklmnopqrstuvwxyz012345"] {
        let spelled = spell(name).expect(name);
        assert_eq!((spelled.written.as_str(), spelled.key.as_str(), &*file_name(&spelled.key)), (name, name, name));
        assert!(MachineName::try_from(name).expect(name).is_ascii_word());
    }
    for (name, file) in [("con", "con_"), ("Nul", "nul_"), ("ＣＯＭ1", "com1_"), ("lpt0", "lpt0_"), ("cons", "cons")]
    {
        assert_eq!(file_name(&spell(name).expect(name).key), file);
    }
}

#[test]
fn names_of_any_language() {
    let same = |a: &str, b: &str| MachineName::try_from(a).expect(a) == MachineName::try_from(b).expect(b);
    assert!(
        same("ＧＰＵ-2", "gpu-2")
            && same("GPU-2", "gpu-2")
            && same("Straße", "STRASSE")
            && same("école", "e\u{301}cole")
    );
    assert!(!same("台式机", "台式機"));
    assert_eq!(MachineName::try_from("ＧＰＵ－２").expect("full width").as_str(), "GPU-2");
    for name in [
        "台式机",
        "PC台式机",
        "PC-서버",
        "コンピューター",
        "PC-घर",
        "2号机",
        "一二三四五六七八九十甲乙丙丁戊己庚辛壬癸子",
    ] {
        MachineName::try_from(name).expect(name);
    }
    let problem = |name: &str| MachineName::try_from(name).map(drop).map_err(|error| error.problem.expect(name));
    assert_eq!(problem("pаypal"), Err(NameProblem::MixedScripts { first: 'p', second: 'а' }));
    assert_eq!(problem("PC-Ελλάδα"), Err(NameProblem::MixedScripts { first: 'P', second: 'Ε' }));
    assert_eq!(problem("一二三四五六七八九十甲乙丙丁戊己庚辛壬癸子丑"), Err(NameProblem::TooLong));
    assert_eq!(problem("a b"), Err(NameProblem::Space));
    assert_eq!(problem("gpu.acme"), Err(NameProblem::NotAllowed { character: '.' }));
    assert_eq!(problem("gpu\u{200b}"), Err(NameProblem::NotAllowed { character: '\u{200b}' }));
    assert_eq!(problem("-gpu"), Err(NameProblem::BadStart { character: '-' }));
    assert_eq!(problem("gpu-"), Err(NameProblem::EndsWithHyphen));
    assert_eq!(problem("Ｈｕｂ"), Err(NameProblem::Reserved));
}
