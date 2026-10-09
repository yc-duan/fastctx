//! The external executable contract, exercised natively by every release runner.

use std::fs;
use std::process::Command;

#[test]
fn official_image_runs_offline_and_repairs_its_placement() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("World's tools 空间");
    fs::create_dir(&directory).unwrap();
    let executable = fastctx_ripgrep::place(&directory).unwrap();
    let original = fs::read(&executable).unwrap();
    let modified = fs::metadata(&executable).unwrap().modified().unwrap();
    fastctx_ripgrep::place(&directory).unwrap();
    assert_eq!(fs::metadata(&executable).unwrap().modified().unwrap(), modified);
    fs::write(&executable, b"interrupted or corrupted placement").unwrap();
    fs::remove_file(directory.join("NOTICE")).unwrap();
    fs::write(directory.join("THIRD_PARTY_LICENSES.md"), b"interrupted placement").unwrap();
    fastctx_ripgrep::place(&directory).unwrap();
    assert_eq!(fs::read(&executable).unwrap(), original);
    assert_eq!(fs::read_to_string(directory.join("NOTICE")).unwrap(), fastctx_ripgrep::licenses::NOTICE);
    assert_eq!(
        fs::read_to_string(directory.join("THIRD_PARTY_LICENSES.md")).unwrap(),
        fastctx_ripgrep::licenses::THIRD_PARTY
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o600)).unwrap();
        fastctx_ripgrep::place(&directory).unwrap();
        assert_eq!(fs::metadata(&executable).unwrap().permissions().mode() & 0o777, 0o700);
    }
    let version = Command::new(&executable).env_clear().arg("--version").output().unwrap();
    assert!(version.status.success(), "{version:?}");
    let version = String::from_utf8(version.stdout).unwrap();
    assert_eq!(version.split_whitespace().take(2).collect::<Vec<_>>(), ["ripgrep", fastctx_ripgrep::VERSION]);
    let input = directory.join("sample.txt");
    fs::write(&input, "needle 42\nother\n").unwrap();
    for pattern in ["^needle [0-9]+$", r"needle (?=42)\d+"] {
        let result = Command::new(&executable)
            .env_clear()
            .args(["--no-config", "--pcre2", "--color=never", "-n", pattern])
            .arg(&input)
            .output()
            .unwrap();
        assert!(result.status.success(), "{result:?}");
        assert_eq!(result.stdout, b"1:needle 42\n");
    }
    let missing = Command::new(&executable).env_clear().args(["--no-config", "absent"]).arg(&input).status().unwrap();
    assert_eq!(missing.code(), Some(1));
}
