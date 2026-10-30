//! Command line behaviour that needs no fault injection.

use std::fs;
use std::process::Command;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_zipshrink"))
}

fn tree(root: &std::path::Path) {
    fs::create_dir_all(root.join("d")).unwrap();
    fs::write(root.join("a.txt"), b"alpha").unwrap();
    fs::write(root.join("d/b.txt"), b"beta".repeat(1000)).unwrap();
}

#[test]
fn create_without_flag_never_deletes() {
    let t = tempfile::tempdir().unwrap();
    let src = t.path().join("src");
    tree(&src);
    let out = t.path().join("o.zip");
    let o = bin().args(["create", "-q", "--min-free", "16M"]).arg(&src).arg("-o").arg(&out).output().unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert!(src.join("a.txt").exists() && src.join("d/b.txt").exists());
    let v = bin().arg("verify").arg(&out).output().unwrap();
    assert!(v.status.success());
    assert!(String::from_utf8_lossy(&v.stdout).contains("2 files matched their stored SHA-256"));
}

#[test]
fn existing_archive_is_not_overwritten() {
    let t = tempfile::tempdir().unwrap();
    let src = t.path().join("src");
    tree(&src);
    let out = t.path().join("o.zip");
    fs::write(&out, b"keep me").unwrap();
    let o = bin().args(["create", "-q", "--min-free", "16M"]).arg(&src).arg("-o").arg(&out).output().unwrap();
    assert_eq!(o.status.code(), Some(1));
    assert_eq!(fs::read(&out).unwrap(), b"keep me");
}

#[test]
fn check_reports_insufficient_space_with_exit_code_4() {
    let t = tempfile::tempdir().unwrap();
    let src = t.path().join("src");
    tree(&src);
    let o = bin().args(["check", "--min-free", "1000T"]).arg(&src).arg("-o").arg(t.path().join("o.zip")).output().unwrap();
    assert_eq!(o.status.code(), Some(4));
    assert!(String::from_utf8_lossy(&o.stdout).contains("does not fit"));
    assert!(!t.path().join("o.zip").exists() && !t.path().join("o.zip.partial").exists());
    let o = bin().args(["create", "--min-free", "1000T"]).arg(&src).arg("-o").arg(t.path().join("o.zip")).output().unwrap();
    assert_eq!(o.status.code(), Some(4));
    assert!(!t.path().join("o.zip.partial").exists(), "refused before touching anything");
}

#[test]
fn verify_fails_on_corruption_with_exit_code_6() {
    let t = tempfile::tempdir().unwrap();
    let src = t.path().join("src");
    tree(&src);
    let out = t.path().join("o.zip");
    assert!(bin().args(["create", "-q", "-l", "0", "--min-free", "16M"]).arg(&src).arg("-o").arg(&out).status().unwrap().success());
    let mut d = fs::read(&out).unwrap();
    let at = d.windows(4).position(|w| w == b"beta").unwrap();
    d[at] = b'X';
    fs::write(&out, d).unwrap();
    let v = bin().arg("verify").arg(&out).output().unwrap();
    assert_eq!(v.status.code(), Some(6));
}

#[test]
fn help_mentions_exit_codes_and_safety() {
    let o = bin().arg("--help").output().unwrap();
    let s = String::from_utf8_lossy(&o.stdout);
    assert!(s.contains("EXIT CODES") && s.contains("--delete-originals"));
    let o = bin().args(["create", "--help"]).output().unwrap();
    assert!(String::from_utf8_lossy(&o.stdout).contains("Permanently delete"));
}

#[test]
fn resume_without_job_is_an_error() {
    let t = tempfile::tempdir().unwrap();
    let o = bin().arg("resume").arg(t.path().join("none.zip")).output().unwrap();
    assert_eq!(o.status.code(), Some(1));
}
