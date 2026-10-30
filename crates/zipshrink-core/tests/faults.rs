//! In-process fault injection. Run with `--features failpoints`.
#![cfg(feature = "failpoints")]

mod common;

use common::*;
use std::fs;
use std::path::PathBuf;
use zipshrink_core::engine::{Outcome, StopKind};
use zipshrink_core::failpoint::{arm, arm_from, disarm_all, set_hook, Action};
use zipshrink_core::{Error, Job, JobOptions};

fn setup() -> (tempfile::TempDir, PathBuf, PathBuf) {
    disarm_all();
    let t = tempfile::tempdir().unwrap();
    let src = t.path().join("GameFiles");
    sample_tree(&src);
    let out = t.path().join("GameFiles.zip");
    (t, src, out)
}

fn del() -> JobOptions {
    JobOptions { delete_originals: true, batch_entries: 2, ..opts() }
}

/// Every original that is gone must be in the archive with the same hash.
fn assert_nothing_lost(
    want: &std::collections::BTreeMap<String, String>,
    src: &std::path::Path,
    archive: &std::collections::BTreeMap<String, String>,
) {
    let present = if src.exists() { expected_entries(src) } else { Default::default() };
    for (name, hash) in want {
        if hash == "dir" {
            continue;
        }
        if present.get(name) != Some(hash) {
            assert_eq!(archive.get(name), Some(hash), "{name} was deleted without a verified archived copy");
        }
    }
}

/// On Unix nothing stops another program writing to a file while it is
/// read, so ZipShrink detects the change and reads the file again.
#[cfg(unix)]
#[test]
fn file_modified_while_being_read_is_reread() {
    let (_t, src, out) = setup();
    let target = src.join("empty.bin");
    let t2 = target.clone();
    let mut calls = 0;
    set_hook("entry_header_written", move || {
        calls += 1;
        if calls == 1 {
            fs::write(&t2, b"changed!").unwrap();
        }
    });
    let r = create_and_run(std::slice::from_ref(&src), &out, JobOptions { threads: 1, ..del() });
    assert_eq!(r.outcome, Outcome::Completed, "{r:?}");
    assert_eq!(archive_entries(&out).get("GameFiles/empty.bin"), Some(&sha(b"changed!")));
    assert!(!target.exists());
}

#[cfg(unix)]
#[test]
fn file_that_keeps_changing_fails_and_is_kept() {
    let (_t, src, out) = setup();
    let target = src.join("empty.bin");
    let t2 = target.clone();
    set_hook("entry_header_written", move || {
        let mut d = fs::read(&t2).unwrap_or_default();
        d.push(b'x');
        let _ = fs::write(&t2, d);
    });
    let r = create_and_run(std::slice::from_ref(&src), &out, JobOptions { threads: 1, ..del() });
    assert_eq!(r.outcome, Outcome::CompletedWithIssues, "{r:?}");
    assert_eq!(r.failed.len(), 1);
    assert!(target.exists());
    assert!(!archive_entries(&out).contains_key("GameFiles/empty.bin"));
}

/// On Windows the source is opened with FILE_SHARE_READ only, so other
/// programs cannot write to it while it is compressed at all.
#[cfg(windows)]
#[test]
fn writers_are_locked_out_while_a_file_is_read() {
    let (_t, src, out) = setup();
    let want = expected_entries(&src);
    let target = src.join("readme.txt");
    let t2 = target.clone();
    let blocked = std::rc::Rc::new(std::cell::Cell::new(0));
    let b2 = blocked.clone();
    set_hook("entry_header_written", move || {
        if let Err(e) = fs::OpenOptions::new().write(true).open(&t2) {
            if e.raw_os_error() == Some(32) {
                b2.set(b2.get() + 1);
            }
        }
    });
    let r = create_and_run(&[src.clone()], &out, JobOptions { threads: 1, ..del() });
    assert_eq!(r.outcome, Outcome::Completed, "{r:?}");
    assert!(blocked.get() >= 1, "the write while readme.txt was open must be refused");
    assert_eq!(archive_entries(&out), want);
}

#[test]
fn file_changed_after_archiving_is_not_deleted() {
    for quick in [false, true] {
        let (_t, src, out) = setup();
        let want = expected_entries(&src);
        let target = src.join("readme.txt");
        let t2 = target.clone();
        let mut calls = 0;
        set_hook("before_delete", move || {
            calls += 1;
            if calls == 2 {
                // readme.txt is the second file deleted; change it, same length.
                fs::write(&t2, b"HELLO, WORLD\n").unwrap();
            }
        });
        let r = create_and_run(std::slice::from_ref(&src), &out, JobOptions { quick_change_check: quick, ..del() });
        assert_eq!(r.outcome, Outcome::CompletedWithIssues, "{r:?}");
        assert_eq!(fs::read(&target).unwrap(), b"HELLO, WORLD\n");
        assert_eq!(r.kept.len(), 1, "{:?}", r.kept);
        let got = archive_entries(&out);
        assert_eq!(got.get("GameFiles/readme.txt"), want.get("GameFiles/readme.txt"), "archive holds the verified version");
        assert_eq!(got, want);
    }
}

#[test]
fn unreadable_file_is_reported_and_kept() {
    let (_t, src, out) = setup();
    arm("open_source", 2, Action::Eio);
    let want = expected_entries(&src);
    let r = create_and_run(std::slice::from_ref(&src), &out, JobOptions { threads: 1, ..del() });
    assert_eq!(r.outcome, Outcome::CompletedWithIssues, "{r:?}");
    assert_eq!(r.failed.len(), 1);
    let failed = &r.failed[0].0;
    assert!(failed.exists(), "the unreadable file is left alone");
    assert_nothing_lost(&want, &src, &archive_entries(&out));
}

#[test]
fn file_that_cannot_be_deleted_is_kept() {
    let (_t, src, out) = setup();
    arm("delete_source", 1, Action::Eio);
    let want = expected_entries(&src);
    let r = create_and_run(std::slice::from_ref(&src), &out, del());
    assert_eq!(r.outcome, Outcome::CompletedWithIssues);
    assert_eq!(r.kept.len(), 1);
    assert!(r.kept[0].1.contains("could not be deleted"));
    assert_eq!(remaining_files(&src).len(), 1);
    assert_eq!(archive_entries(&out), want);
}

#[test]
fn disk_full_stops_safely_and_resumes() {
    for nth in [2, 5, 9, 14] {
        let (_t, src, out) = setup();
        let want = expected_entries(&src);
        arm_from("archive_write", nth, Action::Enospc);
        let r = create_and_run(std::slice::from_ref(&src), &out, del());
        assert!(matches!(r.outcome, Outcome::Stopped { kind: StopKind::DiskFull, resumable: true, .. }), "{nth}: {r:?}");
        disarm_all();
        let r = resume(&out);
        assert_eq!(r.outcome, Outcome::Completed, "{r:?}");
        assert_eq!(archive_entries(&out), want);
        assert!(!src.exists());
    }
}

#[test]
fn data_corrupted_before_verification_stops_without_deleting_that_batch() {
    let (_t, src, out) = setup();
    let want = expected_entries(&src);
    let partial = zipshrink_core::JobPaths::for_output(&out).partial;
    let p2 = partial.clone();
    let mut calls = 0;
    set_hook("batch_synced", move || {
        calls += 1;
        if calls == 3 {
            let mut d = fs::read(&p2).unwrap();
            let n = d.len();
            d[n - 3] ^= 0xff; // inside the last entry of the batch
            fs::write(&p2, d).unwrap();
        }
    });
    let r = create_and_run(std::slice::from_ref(&src), &out, del());
    assert!(matches!(r.outcome, Outcome::Stopped { kind: StopKind::VerifyFailed, .. }), "{r:?}");
    let left = expected_entries(&src);
    assert!(left.len() > 1);
    disarm_all();
    let r = resume(&out);
    assert_eq!(r.outcome, Outcome::Completed, "{r:?}");
    assert_eq!(archive_entries(&out), want);
}

fn last_commit(out: &std::path::Path, file: bool) -> zipshrink_core::journal::Commit {
    use zipshrink_core::journal::{load, Record};
    let l = load(&zipshrink_core::JobPaths::for_output(out).journal).unwrap();
    l.records
        .into_iter()
        .filter_map(|r| match r {
            Record::Commit(c) if c.meta.is_dir != file => Some(c),
            _ => None,
        })
        .next_back()
        .unwrap()
}

fn flip(path: &std::path::Path, at: u64) {
    let mut d = fs::read(path).unwrap();
    d[at as usize] ^= 0x55;
    fs::write(path, d).unwrap();
}

#[test]
fn damaged_partial_halts_pending_deletions() {
    let (_t, src, out) = setup();
    let want = expected_entries(&src);
    arm("journal_committed", 3, Action::Eio);
    let r = create_and_run(std::slice::from_ref(&src), &out, del());
    assert!(matches!(r.outcome, Outcome::Stopped { .. }), "{r:?}");
    disarm_all();
    let before = expected_entries(&src);
    // Corrupt the data of the newest committed file, whose original still exists.
    let c = last_commit(&out, true);
    assert!(src.parent().unwrap().join(&c.meta.name).exists());
    flip(&zipshrink_core::JobPaths::for_output(&out).partial, c.end - 1);
    let mut job = Job::open(&out).unwrap();
    let r = run(&mut job);
    assert!(matches!(r.outcome, Outcome::Stopped { kind: StopKind::Damaged, resumable: false, .. }), "{r:?}");
    drop(job); // releases the journal lock (Windows refuses reads while it is held)
    assert_eq!(expected_entries(&src), before, "nothing more was deleted");
    assert_nothing_lost(&want, &src, &committed_hashes(&out));
}

#[test]
fn damaged_committed_header_is_detected_on_resume() {
    let (_t, src, out) = setup();
    arm("journal_committed", 3, Action::Eio);
    create_and_run(std::slice::from_ref(&src), &out, del());
    disarm_all();
    let c = last_commit(&out, false);
    flip(&zipshrink_core::JobPaths::for_output(&out).partial, c.end - 2); // inside its extra fields
    assert!(matches!(Job::open(&out), Err(Error::Damaged(_))));
}

#[test]
fn truncated_partial_is_reported_as_damaged() {
    let (_t, src, out) = setup();
    arm("journal_committed", 2, Action::Eio);
    create_and_run(std::slice::from_ref(&src), &out, del());
    disarm_all();
    let partial = zipshrink_core::JobPaths::for_output(&out).partial;
    let len = fs::metadata(&partial).unwrap().len();
    fs::OpenOptions::new().write(true).open(&partial).unwrap().set_len(len / 2).unwrap();
    assert!(matches!(Job::open(&out), Err(Error::Damaged(_))));
}

#[test]
fn interrupted_finalization_and_cleanup_resume() {
    for point in ["finalize_cd_partial", "finalize_written", "before_rename", "after_rename", "before_cleanup"] {
        let (_t, src, out) = setup();
        let want = expected_entries(&src);
        arm(point, 1, Action::Eio);
        let r = create_and_run(std::slice::from_ref(&src), &out, del());
        assert!(matches!(r.outcome, Outcome::Stopped { .. }), "{point}: {r:?}");
        disarm_all();
        let r = resume(&out);
        assert_eq!(r.outcome, Outcome::Completed, "{point}: {r:?}");
        assert_eq!(archive_entries(&out), want, "{point}");
        assert!(!src.exists(), "{point}");
    }
}

#[test]
fn interrupted_planning_replans_on_resume() {
    let (_t, src, out) = setup();
    let want = expected_entries(&src);
    arm("before_plan_done", 1, Action::Eio);
    assert!(create(std::slice::from_ref(&src), &out, del()).is_err());
    disarm_all();
    let r = resume(&out);
    assert_eq!(r.outcome, Outcome::Completed, "{r:?}");
    assert_eq!(archive_entries(&out), want);
}

#[test]
fn errors_at_every_transaction_step_never_lose_data() {
    let points = [
        "entry_header_written",
        "entry_data_written",
        "before_archive_sync",
        "batch_synced",
        "before_journal_commit",
        "journal_committed",
        "before_delete",
        "after_delete",
    ];
    for point in points {
        for nth in [1, 3, 6] {
            let (_t, src, out) = setup();
            let want = expected_entries(&src);
            arm(point, nth, Action::Eio);
            let r = create_and_run(std::slice::from_ref(&src), &out, JobOptions { threads: 1, ..del() });
            disarm_all();
            if r.finished() {
                continue; // fewer hits than `nth`
            }
            assert_nothing_lost(&want, &src, &committed_hashes(&out));
            let r = resume(&out);
            assert_eq!(r.outcome, Outcome::Completed, "{point}:{nth}: {r:?}");
            assert_eq!(archive_entries(&out), want, "{point}:{nth}");
            assert!(!src.exists(), "{point}:{nth}");
        }
    }
}
