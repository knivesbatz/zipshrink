//! Kills the real `zipshrink` process at every critical point of the
//! transaction (with `abort()`, which skips all cleanup, as a crash or
//! forced termination does), then resumes and checks that nothing was lost
//! and the final archive reproduces every original exactly.
//!
//! Run with `cargo test -p zipshrink-cli --features failpoints`.
#![cfg(feature = "failpoints")]

use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Output};

fn bin() -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_zipshrink"));
    c.env_remove("ZIPSHRINK_FAILPOINT");
    c
}

fn write(p: &Path, data: &[u8]) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, data).unwrap();
}

fn noise(seed: u64, len: usize) -> Vec<u8> {
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    let mut v = Vec::with_capacity(len + 8);
    while v.len() < len {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        v.extend_from_slice(&x.to_le_bytes());
    }
    v.truncate(len);
    v
}

fn tree(root: &Path) {
    write(&root.join("a.txt"), b"alpha");
    write(&root.join("big.bin"), &noise(1, 3 << 20));
    write(&root.join("zeros.bin"), &vec![0; 2 << 20]);
    for i in 0..12 {
        write(&root.join(format!("sub/{i}/file{i}.dat")), &noise(i + 10, 1000 * i as usize));
    }
    write(&root.join("ünicode/ファイル.txt"), "データ".as_bytes());
    fs::create_dir_all(root.join("empty")).unwrap();
}

fn sha(d: &[u8]) -> String {
    hex::encode(Sha256::digest(d))
}

fn files(root: &Path, prefix: &str, m: &mut BTreeMap<String, String>) {
    if !root.exists() {
        return;
    }
    for e in fs::read_dir(root).unwrap() {
        let e = e.unwrap();
        let name = format!("{prefix}/{}", e.file_name().to_str().unwrap());
        if e.file_type().unwrap().is_dir() {
            m.insert(format!("{name}/"), "dir".into());
            files(&e.path(), &name, m);
        } else {
            m.insert(name, sha(&fs::read(e.path()).unwrap()));
        }
    }
}

fn expected(src: &Path) -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    m.insert("src/".to_string(), "dir".to_string());
    files(src, "src", &mut m);
    m
}

fn archive(p: &Path) -> BTreeMap<String, String> {
    let mut z = zip::ZipArchive::new(fs::File::open(p).unwrap()).unwrap();
    let mut m = BTreeMap::new();
    for i in 0..z.len() {
        let mut f = z.by_index(i).unwrap();
        let name = String::from_utf8(f.name_raw().to_vec()).unwrap();
        if f.is_dir() {
            m.insert(name, "dir".into());
        } else {
            let mut d = Vec::new();
            f.read_to_end(&mut d).unwrap();
            m.insert(name, sha(&d));
        }
    }
    m
}

fn committed(out: &Path) -> BTreeMap<String, String> {
    use zipshrink_core::journal::{load, Record};
    let paths = zipshrink_core::JobPaths::for_output(out);
    let mut m = BTreeMap::new();
    if let Ok(l) = load(&paths.journal) {
        for r in l.records {
            if let Record::Commit(c) = r {
                m.insert(c.meta.name.clone(), c.sha256.map(hex::encode).unwrap_or_else(|| "dir".into()));
            }
        }
    }
    m
}

fn create(src: &Path, out: &Path, failpoint: Option<&str>) -> Output {
    let mut c = bin();
    c.args(["create", "--delete-originals", "--yes", "--min-free", "16M", "-j", "1", "-q", "--batch-entries", "3"])
        .arg(src)
        .arg("--output")
        .arg(out);
    if let Some(f) = failpoint {
        c.env("ZIPSHRINK_FAILPOINT", f);
    }
    c.output().unwrap()
}

fn resume(out: &Path, failpoint: Option<&str>) -> Output {
    let mut c = bin();
    c.args(["resume", "--yes", "-q"]).arg(out);
    if let Some(f) = failpoint {
        c.env("ZIPSHRINK_FAILPOINT", f);
    }
    c.output().unwrap()
}

/// Every original that is gone must be committed with the same hash.
fn assert_no_loss(want: &BTreeMap<String, String>, src: &Path, out: &Path, ctx: &str) {
    let now = expected(src);
    let safe = if out.exists() { archive(out) } else { committed(out) };
    for (name, hash) in want {
        if hash != "dir" && now.get(name) != Some(hash) {
            assert_eq!(safe.get(name), Some(hash), "{ctx}: {name} is gone but not safely archived");
        }
    }
}

const POINTS: &[(&str, &[u32])] = &[
    ("before_plan_done", &[1]),
    ("entry_header_written", &[1, 4, 9]),
    ("entry_data_written", &[1, 5, 12]),
    ("before_archive_sync", &[1, 3]),
    ("batch_synced", &[1, 3, 6]),
    ("before_journal_commit", &[1, 4]),
    ("journal_torn", &[1, 2, 5]),
    ("journal_committed", &[1, 3, 6]),
    ("before_delete", &[1, 4, 9]),
    ("after_delete", &[1, 4, 9]),
    ("finalize_cd_partial", &[1]),
    ("finalize_written", &[1]),
    ("before_rename", &[1]),
    ("after_rename", &[1]),
    ("before_cleanup", &[1]),
];

#[test]
fn crash_at_every_critical_point_then_resume() {
    let mut crashed = 0;
    for (point, nths) in POINTS {
        for nth in *nths {
            let ctx = format!("{point}:{nth}");
            let t = tempfile::tempdir().unwrap();
            let src = t.path().join("src");
            tree(&src);
            let want = expected(&src);
            let out = t.path().join("out.zip");
            let o = create(&src, &out, Some(&ctx));
            if o.status.success() {
                continue; // the point was hit fewer than `nth` times
            }
            crashed += 1;
            assert!(String::from_utf8_lossy(&o.stderr).contains("failpoint"), "{ctx}: {}", String::from_utf8_lossy(&o.stderr));
            assert!(!out.exists() || *point == "after_rename" || *point == "before_cleanup", "{ctx}: archive finalized early");
            assert_no_loss(&want, &src, &out, &ctx);
            if !t.path().join("out.zip.partial.journal").exists() {
                // Crashed before the job existed: nothing changed, start again.
                assert_eq!(expected(&src), want, "{ctx}");
                let o = create(&src, &out, None);
                assert!(o.status.success(), "{ctx}: {}", String::from_utf8_lossy(&o.stderr));
                assert_eq!(archive(&out), want, "{ctx}");
                continue;
            }
            let r = resume(&out, None);
            assert!(r.status.success(), "{ctx}: resume failed: {}", String::from_utf8_lossy(&r.stderr));
            assert_eq!(archive(&out), want, "{ctx}");
            assert!(!src.exists(), "{ctx}: source folder should be gone");
            assert!(!t.path().join("out.zip.partial").exists() && !t.path().join("out.zip.partial.journal").exists(), "{ctx}");
            let v = bin().arg("verify").arg(&out).output().unwrap();
            assert!(v.status.success(), "{ctx}");
        }
    }
    assert!(crashed >= 30, "only {crashed} crash points were reached");
}

#[test]
fn repeated_crashes_during_resume_still_converge() {
    let t = tempfile::tempdir().unwrap();
    let src = t.path().join("src");
    tree(&src);
    let want = expected(&src);
    let out = t.path().join("out.zip");
    let points = [
        "batch_synced:2",
        "before_delete:2",
        "journal_torn:1",
        "after_delete:3",
        "entry_data_written:2",
        "finalize_written:1",
        "before_cleanup:1",
    ];
    let o = create(&src, &out, Some("before_delete:3"));
    assert!(!o.status.success());
    let mut done = false;
    for (i, p) in points.iter().cycle().take(40).enumerate() {
        assert_no_loss(&want, &src, &out, p);
        let r = resume(&out, Some(p));
        if r.status.success() {
            done = true;
            break;
        }
        assert!(String::from_utf8_lossy(&r.stderr).contains("failpoint"), "round {i} {p}: {}", String::from_utf8_lossy(&r.stderr));
    }
    assert!(done, "never finished");
    assert_eq!(archive(&out), want);
    assert!(!src.exists());
}

#[test]
fn injected_disk_full_exits_with_space_code_and_resumes() {
    let t = tempfile::tempdir().unwrap();
    let src = t.path().join("src");
    tree(&src);
    let want = expected(&src);
    let out = t.path().join("out.zip");
    let o = create(&src, &out, Some("archive_write:6+=enospc"));
    assert_eq!(o.status.code(), Some(4), "{}", String::from_utf8_lossy(&o.stderr));
    assert!(String::from_utf8_lossy(&o.stderr).contains("zipshrink resume"));
    assert_no_loss(&want, &src, &out, "enospc");
    let r = resume(&out, None);
    assert!(r.status.success());
    assert_eq!(archive(&out), want);
}
