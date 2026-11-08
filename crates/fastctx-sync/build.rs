//! Checks and compiles the closed inventory without accessing service data.

use std::{collections::BTreeSet, env, fs, path::PathBuf};

fn main() {
    let mut input = String::new();
    for source in [
        "src/registry/legacy.tsv",
        "src/registry/legacy_families.tsv",
        "src/registry/legacy_files.tsv",
        "src/registry/legacy_browser.tsv",
    ] {
        println!("cargo:rerun-if-changed={source}");
        input.push_str(&fs::read_to_string(source).expect("read persistence inventory"));
        input.push('\n');
    }
    let mut output = String::from("pub const ENTRIES: &[LegacyEntry] = &[\n");
    let mut keys = BTreeSet::new();
    for (line_number, line) in input.lines().enumerate() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let cells: Vec<_> = line.split('\t').collect();
        assert_eq!(cells.len(), 6, "inventory line {} needs six columns", line_number + 1);
        let [surface, selector, class, owner, stage, note] = <[&str; 6]>::try_from(cells).unwrap();
        assert!(keys.insert((surface, selector)), "duplicate inventory entry {surface} {selector}");
        assert!(!owner.is_empty() && !note.is_empty(), "inventory entries require an owner and disposition");
        assert!(
            ["Table", "Family", "File", "Browser", "Container", "External", "Migration", "Reference"]
                .contains(&surface)
        );
        assert!(
            ["Foundation", "Ingestion", "Projection", "Assets", "Client", "Migration", "Maintenance"].contains(&stage)
        );
        let classification = match class.strip_prefix("ByKind:") {
            Some(kind) if !kind.is_empty() => format!("Classification::ByKind({kind:?})"),
            _ => {
                assert!(["Asset", "Runtime", "Derived", "Scratch"].contains(&class), "unknown storage class {class}");
                format!("Classification::Fixed(DataClass::{class})")
            }
        };
        output.push_str(&format!("LegacyEntry {{ surface: Surface::{surface}, selector: {selector:?}, classification: {classification}, owner: {owner:?}, stage: Stage::{stage}, note: {note:?} }},\n"));
    }
    output.push_str("];\n");
    let path = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo output directory")).join("legacy_registry.rs");
    fs::write(path, output).expect("write checked persistence registry");
}
