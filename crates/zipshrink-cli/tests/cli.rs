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

/// Incompressible bytes, so archive sizes are predictable.
fn noise(len: usize, seed: u64) -> Vec<u8> {
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as u8
        })
        .collect()
}

fn create(args: &[&str], src: &std::path::Path, out: &std::path::Path) -> std::process::Output {
    bin().arg("create").args(["--min-free", "16M"]).args(args).arg(src).arg("-o").arg(out).output().unwrap()
}

#[test]
fn part_size_splits_into_zips_of_that_size() {
    let t = tempfile::tempdir().unwrap();
    let src = t.path().join("src");
    fs::create_dir_all(&src).unwrap();
    for (i, n) in ["a.bin", "b.bin", "c.bin", "d.bin"].iter().enumerate() {
        fs::write(src.join(n), noise(100 << 10, i as u64)).unwrap();
    }
    let out = t.path().join("o.zip");
    let o = create(&["--part-size", "250K"], &src, &out);
    let stdout = String::from_utf8_lossy(&o.stdout);
    assert!(o.status.success(), "{stdout}\n{}", String::from_utf8_lossy(&o.stderr));
    assert!(stdout.contains("Split into 2 separate ZIP files"), "{stdout}");
    assert!(!out.exists());
    for p in ["o.part1.zip", "o.part2.zip"] {
        let p = t.path().join(p);
        assert!(fs::metadata(&p).unwrap().len() <= 250 << 10, "{} is too large", p.display());
        let v = bin().arg("verify").arg(&p).output().unwrap();
        assert!(String::from_utf8_lossy(&v.stdout).contains("2 files matched"), "{}", String::from_utf8_lossy(&v.stdout));
    }
    assert!(!t.path().join("o.part3.zip").exists());
}

#[test]
fn part_size_that_fits_everything_does_not_split() {
    let t = tempfile::tempdir().unwrap();
    let src = t.path().join("src");
    tree(&src);
    let out = t.path().join("o.zip");
    let o = create(&["-s", "1G"], &src, &out);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert!(String::from_utf8_lossy(&o.stdout).contains("Everything fits in one ZIP file of that size, so it is not split."));
    assert!(out.exists() && !t.path().join("o.part1.zip").exists());
}

#[test]
fn part_size_gives_a_larger_file_its_own_zip() {
    let t = tempfile::tempdir().unwrap();
    let src = t.path().join("src");
    fs::create_dir_all(&src).unwrap();
    fs::write(src.join("a-big.bin"), noise(300 << 10, 1)).unwrap();
    fs::write(src.join("b.txt"), b"small").unwrap();
    fs::write(src.join("c.txt"), b"small too").unwrap();
    let out = t.path().join("o.zip");
    let o = bin().arg("check").args(["--min-free", "16M", "--part-size", "100K"]).arg(&src).arg("-o").arg(&out).output().unwrap();
    let stdout = String::from_utf8_lossy(&o.stdout);
    assert!(o.status.success(), "{stdout}\n{}", String::from_utf8_lossy(&o.stderr));
    assert!(stdout.contains("Split into 2 separate ZIP files"), "{stdout}");
    assert!(stdout.contains("1 of them hold a single file larger than 100 KiB; a file is never cut in two."), "{stdout}");
}

#[test]
fn part_size_is_checked() {
    let t = tempfile::tempdir().unwrap();
    let src = t.path().join("src");
    tree(&src);
    let out = t.path().join("o.zip");
    let o = create(&["--part-size", "1M", "--parts", "3"], &src, &out);
    assert_eq!(o.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&o.stderr).contains("cannot be used with"), "{}", String::from_utf8_lossy(&o.stderr));
    let o = create(&["--part-size", "63K"], &src, &out);
    assert_eq!(o.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&o.stderr).contains("at least 64K"));
    assert!(!out.exists());
}

#[test]
fn update_help_says_it_installs() {
    let o = bin().args(["update", "--help"]).output().unwrap();
    let s = String::from_utf8_lossy(&o.stdout);
    assert!(o.status.success() && s.contains("--check"), "{s}");
}
