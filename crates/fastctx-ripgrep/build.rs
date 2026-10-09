use std::env;
use std::fs;
use std::io::Read as _;
use std::path::PathBuf;

use sha2::{Digest as _, Sha256};

const DEVELOPMENT: &str = "This is a development build of fastctx. It does not embed third-party notices; \
release builds embed the complete notices that scripts/notices.py generates for their lockfile.\n";

fn main() {
    let vendor = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("vendor");
    let manifest = vendor.join("manifest.json");
    println!("cargo:rerun-if-changed={}", manifest.display());
    let manifest: serde_json::Value = serde_json::from_slice(&fs::read(manifest).unwrap()).unwrap();
    let target = env::var("TARGET").unwrap();
    // GNU development builds use the same static payload as Linux releases.
    let asset = target.replace("-unknown-linux-gnu", "-unknown-linux-musl");
    let entry = manifest["targets"].get(&asset).unwrap_or_else(|| panic!("no bundled ripgrep for {target}"));
    let compressed = vendor.join(format!("{asset}.gz"));
    println!("cargo:rerun-if-changed={}", compressed.display());
    let mut bytes = Vec::new();
    flate2::read::GzDecoder::new(fs::File::open(compressed).unwrap()).read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes.len() as u64, entry["size"].as_u64().unwrap(), "ripgrep size for {target}");
    assert_eq!(
        format!("{:x}", Sha256::digest(&bytes)),
        entry["sha256"].as_str().unwrap(),
        "ripgrep SHA-256 for {target}"
    );
    fs::write(PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("rg"), bytes).unwrap();
    // Release builds embed the notices scripts/notices.py generated for this
    // lockfile, and packaging rejects an executable without them. A
    // development build says plainly that it carries none.
    println!("cargo:rerun-if-env-changed=FASTCTX_THIRD_PARTY_LICENSES");
    let embedded = PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("THIRD_PARTY_LICENSES.md");
    match env::var_os("FASTCTX_THIRD_PARTY_LICENSES").map(PathBuf::from) {
        Some(notices) => {
            println!("cargo:rerun-if-changed={}", notices.display());
            fs::copy(&notices, embedded).unwrap_or_else(|error| panic!("reading {}: {error}", notices.display()));
        }
        None => fs::write(embedded, DEVELOPMENT).unwrap(),
    }
    println!("cargo:rustc-env=FASTCTX_RIPGREP_VERSION={}", manifest["version"].as_str().unwrap());
}
