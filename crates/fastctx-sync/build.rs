//! Compiles the closed inventory; additions require baseline source evidence.

use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    path::PathBuf,
};
fn rows(path: &str) -> Vec<Vec<String>> {
    println!("cargo:rerun-if-changed={path}");
    fs::read_to_string(path)
        .expect("read persistence inventory")
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| line.split('\t').map(str::to_owned).collect())
        .collect()
}
fn main() {
    let baseline: BTreeMap<_, _> = rows("src/registry/legacy_baseline.tsv")
        .into_iter()
        .map(|row| ((row[0].clone(), row[1].clone()), row[2].clone()))
        .collect();
    let additions: BTreeSet<_> = rows("src/registry/legacy_additions.tsv")
        .into_iter()
        .map(|row| {
            assert_eq!(row.len(), 6, "an added legacy entry needs complete baseline evidence");
            assert!(!row[2..].iter().any(String::is_empty), "baseline evidence cannot be empty");
            (row[0].clone(), row[1].clone())
        })
        .collect();
    let mut output = String::from("pub const ENTRIES: &[LegacyEntry] = &[\n");
    let mut keys = BTreeSet::new();
    for source in [
        "src/registry/legacy.tsv",
        "src/registry/legacy_families.tsv",
        "src/registry/legacy_files.tsv",
        "src/registry/legacy_browser.tsv",
    ] {
        for row in rows(source) {
            assert_eq!(row.len(), 7, "inventory rows need seven columns");
            let [surface, selector, class, owner, stage, decision, note] = <[String; 7]>::try_from(row).unwrap();
            let key = (surface.clone(), selector.clone());
            assert!(keys.insert(key.clone()), "duplicate inventory entry {surface} {selector}");
            match baseline.get(&key) {
                Some(original) => assert_eq!(original, &owner, "a legacy owner must retain its source evidence"),
                None => {
                    assert!(additions.contains(&key), "new legacy entry needs baseline source evidence: {selector}")
                }
            }
            assert!(!owner.is_empty() && !note.is_empty(), "inventory entries require owner and disposition");
            assert!(
                ["Table", "Family", "File", "Browser", "Container", "External", "Migration", "Reference"]
                    .contains(&surface.as_str())
            );
            assert!(
                ["Foundation", "Ingestion", "Projection", "Assets", "Client", "Migration", "Maintenance"]
                    .contains(&stage.as_str())
            );
            assert!(["Confirmed", "PendingUser"].contains(&decision.as_str()));
            let classification = match class.strip_prefix("ByKind:") {
                Some(kind) if !kind.is_empty() => format!("Classification::ByKind({kind:?})"),
                _ => {
                    assert!(["Asset", "Runtime", "Derived", "Scratch"].contains(&class.as_str()));
                    format!("Classification::Fixed(DataClass::{class})")
                }
            };
            output.push_str(&format!("LegacyEntry {{ surface: Surface::{surface}, selector: {selector:?}, classification: {classification}, owner: {owner:?}, stage: Stage::{stage}, decision: DecisionState::{decision}, note: {note:?} }},\n"));
        }
    }
    output.push_str("];\n");
    fs::write(
        PathBuf::from(env::var_os("OUT_DIR").expect("Cargo output directory")).join("legacy_registry.rs"),
        output,
    )
    .expect("write checked persistence registry");
}
