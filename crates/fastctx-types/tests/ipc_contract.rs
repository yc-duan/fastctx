//! A reviewed wire declaration baseline. Changes to nested types count too;
//! comparing only Hello, Call and Reply misses the most dangerous breaks.

use std::collections::BTreeMap;
use std::path::Path;

use quote::ToTokens;

fn declarations(dir: &Path, root: &Path, out: &mut BTreeMap<String, String>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            declarations(&path, root, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            let source = std::fs::read_to_string(&path).unwrap();
            let source = source
                .lines()
                .filter(|line| !line.trim_start().starts_with("///") && !line.trim_start().starts_with("//!"))
                .collect::<Vec<_>>()
                .join("\n");
            let file = syn::parse_file(&source).unwrap();
            for item in file.items {
                let name = match &item {
                    syn::Item::Struct(item) => item.ident.to_string(),
                    syn::Item::Enum(item) => item.ident.to_string(),
                    syn::Item::Type(item) => item.ident.to_string(),
                    syn::Item::Const(item) if path.file_name().unwrap() == "names.rs" => item.ident.to_string(),
                    syn::Item::Fn(item) if path.file_name().unwrap() == "names.rs" => item.sig.ident.to_string(),
                    // Names use a macro and custom serde conversion. Freeze
                    // both its definition and each invocation/validation rule.
                    syn::Item::Macro(item) => {
                        item.ident.as_ref().map(ToString::to_string).unwrap_or_else(|| item.mac.tokens.to_string())
                    }
                    syn::Item::Impl(item)
                        if item.trait_.as_ref().is_some_and(|(_, path, _)| {
                            path.segments.last().is_some_and(|s| s.ident == "Serialize" || s.ident == "Deserialize")
                        }) =>
                    {
                        format!(
                            "impl {} for {}",
                            item.trait_.as_ref().unwrap().1.to_token_stream(),
                            item.self_ty.to_token_stream()
                        )
                    }
                    _ => continue,
                };
                let key = format!("{}::{name}", path.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/"));
                assert!(out.insert(key, item.into_token_stream().to_string()).is_none());
            }
        }
    }
}

#[test]
fn ipc_wire_declarations_are_reviewed() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut actual = BTreeMap::new();
    declarations(&root.join("src"), &root.join("src"), &mut actual);
    let snapshot = root.join(format!("tests/contracts/protocol-{}.json", fastctx_types::ipc::PROTOCOL));
    if std::env::var_os("FASTCTX_UPDATE_IPC_CONTRACT").is_some() {
        std::fs::create_dir_all(snapshot.parent().unwrap()).unwrap();
        std::fs::write(&snapshot, serde_json::to_string_pretty(&actual).unwrap() + "\n").unwrap();
        panic!(
            "wrote {}; review compatibility and run the historical-reader checks before accepting this baseline",
            snapshot.display()
        );
    }
    let expected: BTreeMap<String, String> = serde_json::from_str(
        &std::fs::read_to_string(&snapshot).expect("a new protocol needs a reviewed contract baseline"),
    )
    .unwrap();
    let changed: Vec<_> =
        actual.keys().chain(expected.keys()).filter(|key| actual.get(*key) != expected.get(*key)).collect();
    assert!(
        changed.is_empty(),
        "IPC declarations changed: {changed:?}. Audit old readers, bump PROTOCOL for incompatible changes, and update renewal_legacy before reviewing a new baseline. Run scripts/check-ipc-contract.py."
    );
}
