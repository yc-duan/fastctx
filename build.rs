use flate2::read::GzDecoder;
use sha2::{Digest, Sha256};
use std::env;
use std::fs;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use tar::Archive;

const RELEASE_TAG: &str = "chromium/7763";
const LOCAL_ARCHIVE_ENV: &str = "FASTCTX_PDFIUM_ARCHIVE";
const DISTRIBUTION_ENV: &str = "FASTCTX_DISTRIBUTION";
const MAX_ARCHIVE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_LIBRARY_BYTES: u64 = 64 * 1024 * 1024;

struct Artifact {
    asset: &'static str,
    archive_sha256: &'static str,
    member: &'static str,
    library_sha256: &'static str,
    filename: &'static str,
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed={DISTRIBUTION_ENV}");
    println!("cargo:rerun-if-env-changed=CARGO_FEATURE_PDF");
    if env::var_os("CARGO_FEATURE_PDF").is_none() {
        return;
    }
    println!("cargo:rerun-if-env-changed={LOCAL_ARCHIVE_ENV}");

    let target = env::var("TARGET").expect("Cargo did not provide TARGET");
    let target_env = format!(
        "{LOCAL_ARCHIVE_ENV}_{}",
        target.replace('-', "_").to_ascii_uppercase()
    );
    println!("cargo:rerun-if-env-changed={target_env}");

    let artifact = artifact_for_target(&target).unwrap_or_else(|| {
        panic!(
            "bundled PDF support does not have a pinned Pdfium artifact for target {target}; supported targets are Windows x64, Linux x64, macOS x64, and macOS arm64"
        )
    });
    let archive_bytes = load_archive(&artifact, &target_env);
    verify_sha256(
        &archive_bytes,
        artifact.archive_sha256,
        "Pdfium release archive",
    );
    let library_bytes = extract_member(&archive_bytes, artifact.member);
    verify_sha256(
        &library_bytes,
        artifact.library_sha256,
        "Pdfium dynamic library",
    );

    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo did not provide OUT_DIR"));
    let embedded_dir = out_dir.join("bundled-pdfium");
    fs::create_dir_all(&embedded_dir).expect("failed to create Pdfium build output directory");
    let library_path = embedded_dir.join(artifact.filename);
    fs::write(&library_path, library_bytes).expect("failed to write extracted Pdfium library");
    write_generated_module(&out_dir, &library_path, &artifact);
}

fn artifact_for_target(target: &str) -> Option<Artifact> {
    let artifact = match target {
        "x86_64-pc-windows-msvc" | "x86_64-pc-windows-gnu" => Artifact {
            asset: "pdfium-win-x64.tgz",
            archive_sha256: "45c4cc5d052ef8ec6380b946b548a76100f4675e38362000a4c732e16d5e8eda",
            member: "bin/pdfium.dll",
            library_sha256: "a63949dc46a7314bba619ac6cc1b3849627e137f542ae31b2b36b302841f77ae",
            filename: "pdfium.dll",
        },
        "x86_64-unknown-linux-gnu" => Artifact {
            asset: "pdfium-linux-x64.tgz",
            archive_sha256: "e3f0c66b2daad710cb6c8edd4a8c45c8902995e359dc0775917fc16e2e56349d",
            member: "lib/libpdfium.so",
            library_sha256: "9167f6d9190f217fab5bfb864620108e280c124b7f7762cc4ef66e1078e0ec62",
            filename: "libpdfium.so",
        },
        "x86_64-apple-darwin" => Artifact {
            asset: "pdfium-mac-x64.tgz",
            archive_sha256: "f455e0868ef7e5174a315de8789ee2b7a5544638d0ac7a3312ea7b68ebbc99cb",
            member: "lib/libpdfium.dylib",
            library_sha256: "b67d8bc289bf9916f697add53b163730ff22243ea896e97f942e09cb634e8a14",
            filename: "libpdfium.dylib",
        },
        "aarch64-apple-darwin" => Artifact {
            asset: "pdfium-mac-arm64.tgz",
            archive_sha256: "9acf49e46c68992cd40810e88264b1ad171805d02fd41c4cca336aad6653b333",
            member: "lib/libpdfium.dylib",
            library_sha256: "0501a43035c44ccd498d77c1bf7fb8aa88facdd0963d423f51cfea2d4d46f52b",
            filename: "libpdfium.dylib",
        },
        _ => return None,
    };
    Some(artifact)
}

fn load_archive(artifact: &Artifact, target_env: &str) -> Vec<u8> {
    if let Some(path) = env::var_os(target_env).or_else(|| env::var_os(LOCAL_ARCHIVE_ENV)) {
        println!("cargo:rerun-if-changed={}", Path::new(&path).display());
        let file = fs::File::open(&path).unwrap_or_else(|error| {
            panic!(
                "failed to read local Pdfium archive {}: {error}",
                Path::new(&path).display()
            )
        });
        return read_limited(file, MAX_ARCHIVE_BYTES, "local Pdfium archive");
    }

    let url = format!(
        "https://github.com/bblanchon/pdfium-binaries/releases/download/{RELEASE_TAG}/{}",
        artifact.asset
    );
    let response = ureq::get(&url)
        .timeout(std::time::Duration::from_secs(150))
        .call()
        .unwrap_or_else(|error| {
            panic!(
                "failed to download pinned Pdfium archive from {url}: {error}. For an offline build, set {LOCAL_ARCHIVE_ENV} to the matching archive path"
            )
        });
    read_limited(
        response.into_reader(),
        MAX_ARCHIVE_BYTES,
        "downloaded Pdfium archive",
    )
}

fn read_limited(mut reader: impl Read, limit: u64, label: &str) -> Vec<u8> {
    let mut bytes = Vec::new();
    reader
        .by_ref()
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .unwrap_or_else(|error| panic!("failed to read {label}: {error}"));
    assert!(
        bytes.len() as u64 <= limit,
        "{label} exceeds the {} MiB build safety limit",
        limit / (1024 * 1024)
    );
    bytes
}

fn verify_sha256(bytes: &[u8], expected: &str, label: &str) {
    let actual = hex::encode(Sha256::digest(bytes));
    assert_eq!(
        actual, expected,
        "{label} SHA-256 mismatch; expected {expected}, got {actual}"
    );
}

fn extract_member(archive_bytes: &[u8], expected_member: &str) -> Vec<u8> {
    let decoder = GzDecoder::new(Cursor::new(archive_bytes));
    let mut archive = Archive::new(decoder);
    let mut found = None;
    for entry in archive
        .entries()
        .expect("failed to read Pdfium tar archive")
    {
        let mut entry = entry.expect("failed to read an entry from Pdfium tar archive");
        let path = entry
            .path()
            .expect("Pdfium archive contains an invalid path")
            .into_owned();
        assert!(
            !path.is_absolute()
                && !path
                    .components()
                    .any(|part| matches!(part, std::path::Component::ParentDir)),
            "Pdfium archive contains an unsafe path: {}",
            path.display()
        );
        if path == Path::new(expected_member) {
            assert!(
                found.is_none(),
                "Pdfium archive contains the library more than once"
            );
            assert!(
                entry.size() <= MAX_LIBRARY_BYTES,
                "Pdfium dynamic library exceeds the {} MiB build safety limit",
                MAX_LIBRARY_BYTES / (1024 * 1024)
            );
            let mut bytes = Vec::new();
            entry
                .by_ref()
                .take(MAX_LIBRARY_BYTES + 1)
                .read_to_end(&mut bytes)
                .expect("failed to extract Pdfium dynamic library");
            assert!(
                bytes.len() as u64 <= MAX_LIBRARY_BYTES,
                "Pdfium dynamic library exceeds the {} MiB build safety limit",
                MAX_LIBRARY_BYTES / (1024 * 1024)
            );
            found = Some(bytes);
        }
    }
    found.unwrap_or_else(|| panic!("Pdfium archive did not contain {expected_member}"))
}

fn write_generated_module(out_dir: &Path, library_path: &Path, artifact: &Artifact) {
    let path_literal = format!("{:?}", library_path.to_string_lossy());
    let source = format!(
        "pub const PDFIUM_BYTES: &[u8] = include_bytes!({path_literal});\n\
         pub const PDFIUM_FILENAME: &str = {:?};\n\
         pub const PDFIUM_SHA256: &str = {:?};\n\
         pub const PDFIUM_RELEASE_TAG: &str = {:?};\n",
        artifact.filename, artifact.library_sha256, RELEASE_TAG
    );
    fs::write(out_dir.join("pdfium_embedded.rs"), source)
        .expect("failed to generate Pdfium embedding module");
}
