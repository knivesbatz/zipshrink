mod common;

use common::*;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use zipshrink_core::engine::{Outcome, StopKind};
use zipshrink_core::progress::{Event, Silent};
use zipshrink_core::{scan, space, verify, Error, Job, JobOptions};

fn setup() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let t = tempfile::tempdir().unwrap();
    let src = t.path().join("GameFiles");
    sample_tree(&src);
    let out = t.path().join("out").join("GameFiles.zip");
    fs::create_dir_all(out.parent().unwrap()).unwrap();
    (t, src, out)
}

#[test]
fn creates_archive_and_keeps_originals_by_default() {
    let (_t, src, out) = setup();
    let want = expected_entries(&src);
    let r = create_and_run(std::slice::from_ref(&src), &out, opts());
    assert_eq!(r.outcome, Outcome::Completed, "{r:?}");
    assert_eq!(archive_entries(&out), want);
    assert_eq!(expected_entries(&src), want, "originals must be untouched");
    assert_eq!(r.originals_deleted, 0);
    let v = r.verification.unwrap();
    assert!(v.ok() && v.data_checked);
    assert_eq!(v.sha_checked, want.values().filter(|v| *v != "dir").count() as u64);
    assert!(!r.paths.partial.exists() && !r.paths.journal.exists());
}

#[test]
fn deletes_originals_only_when_asked_and_reclaims_space() {
    let (_t, src, out) = setup();
    let want = expected_entries(&src);
    let total: u64 = walk_sizes(&src);
    let r = create_and_run(std::slice::from_ref(&src), &out, JobOptions { delete_originals: true, ..opts() });
    assert_eq!(r.outcome, Outcome::Completed, "{r:?}");
    assert_eq!(archive_entries(&out), want);
    assert!(!src.exists(), "the emptied source folder is removed");
    assert_eq!(r.reclaimed, total);
    assert_eq!(r.originals_deleted, want.values().filter(|v| *v != "dir").count() as u64);
}

fn walk_sizes(p: &std::path::Path) -> u64 {
    remaining_files(p).iter().map(|f| fs::metadata(p.join(f)).unwrap().len()).sum()
}

#[test]
fn every_compression_level_round_trips() {
    for level in [0, 1, 6, 9] {
        let (_t, src, out) = setup();
        let want = expected_entries(&src);
        let r = create_and_run(std::slice::from_ref(&src), &out, JobOptions { level, ..opts() });
        assert_eq!(r.outcome, Outcome::Completed);
        assert_eq!(archive_entries(&out), want, "level {level}");
    }
}

#[test]
fn multiple_sources_with_the_same_name_get_unique_names() {
    let t = tempfile::tempdir().unwrap();
    let a = t.path().join("x").join("data");
    let b = t.path().join("y").join("data");
    let f = t.path().join("z").join("data");
    write(&a.join("one.txt"), b"1");
    write(&b.join("two.txt"), b"2");
    write(&f, b"file named data");
    let out = t.path().join("multi.zip");
    let r = create_and_run(&[a, b, f], &out, opts());
    assert_eq!(r.outcome, Outcome::Completed);
    let names: Vec<String> = archive_entries(&out).into_keys().collect();
    assert_eq!(names, vec!["data (2)/", "data (2)/two.txt", "data (3)", "data/", "data/one.txt"]);
    assert_eq!(r.warnings.len(), 2);
}

#[test]
fn thousands_of_files_with_and_without_worker_threads() {
    for threads in [1, 4] {
        let t = tempfile::tempdir().unwrap();
        let src = t.path().join("many");
        for i in 0..3000u64 {
            write(&src.join(format!("d{}/f{i}.txt", i % 37)), &noise(i, (i % 5000) as usize));
        }
        let want = expected_entries(&src);
        let out = t.path().join("many.zip");
        let r = create_and_run(std::slice::from_ref(&src), &out, JobOptions { threads, delete_originals: true, ..opts() });
        assert_eq!(r.outcome, Outcome::Completed, "{r:?}");
        assert_eq!(r.files_archived, 3000);
        assert_eq!(archive_entries(&out), want);
        assert!(!src.exists());
    }
}

#[test]
fn large_file_streams_and_forced_zip64_is_readable() {
    let t = tempfile::tempdir().unwrap();
    let src = t.path().join("big");
    write(&src.join("noise.bin"), &noise(9, 40 << 20));
    let zeros = vec![0u8; 120 << 20];
    write(&src.join("zeros.bin"), &zeros);
    drop(zeros);
    let want = expected_entries(&src);
    let out = t.path().join("big.zip");
    let r = create_and_run(std::slice::from_ref(&src), &out, JobOptions { force_zip64: true, ..opts() });
    assert_eq!(r.outcome, Outcome::Completed, "{r:?}");
    assert_eq!(archive_entries(&out), want);
    assert!(r.archive_size < (45 << 20), "zeros compress");
}

#[test]
fn a_single_file_can_be_the_source() {
    let t = tempfile::tempdir().unwrap();
    let f = t.path().join("movie.mkv");
    write(&f, &noise(4, 1 << 20));
    let want = sha(&fs::read(&f).unwrap());
    let out = t.path().join("movie.zip");
    let r = create_and_run(std::slice::from_ref(&f), &out, JobOptions { delete_originals: true, ..opts() });
    assert_eq!(r.outcome, Outcome::Completed);
    assert_eq!(archive_entries(&out).get("movie.mkv"), Some(&want));
    assert!(!f.exists());
}

#[cfg(unix)]
#[test]
fn symlinks_are_skipped_never_followed_never_deleted() {
    let t = tempfile::tempdir().unwrap();
    let outside = t.path().join("outside");
    write(&outside.join("precious.txt"), b"do not touch");
    let src = t.path().join("src");
    write(&src.join("real.txt"), b"real");
    std::os::unix::fs::symlink(&outside, src.join("link-to-dir")).unwrap();
    std::os::unix::fs::symlink(outside.join("precious.txt"), src.join("link-to-file")).unwrap();
    let out = t.path().join("s.zip");
    let r = create_and_run(std::slice::from_ref(&src), &out, JobOptions { delete_originals: true, ..opts() });
    assert_eq!(r.outcome, Outcome::CompletedWithIssues);
    assert_eq!(r.skipped.len(), 2);
    let names: Vec<String> = archive_entries(&out).into_keys().collect();
    assert_eq!(names, vec!["src/", "src/real.txt"]);
    assert_eq!(fs::read(outside.join("precious.txt")).unwrap(), b"do not touch");
    assert!(fs::symlink_metadata(src.join("link-to-dir")).is_ok(), "links stay");
    assert!(!src.join("real.txt").exists());
}

#[cfg(unix)]
#[test]
fn a_symlink_as_the_selected_source_is_refused() {
    let t = tempfile::tempdir().unwrap();
    let real = t.path().join("real");
    write(&real.join("a"), b"a");
    let link = t.path().join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let err = create(&[link], &t.path().join("x.zip"), opts()).err().unwrap();
    assert!(matches!(err, Error::Invalid(_)), "{err}");
}

#[test]
fn read_only_files_are_kept_unless_allowed() {
    let (_t, src, out) = setup();
    let ro = src.join("readme.txt");
    let mut p = fs::metadata(&ro).unwrap().permissions();
    p.set_readonly(true);
    fs::set_permissions(&ro, p).unwrap();
    let want = expected_entries(&src);
    let r = create_and_run(std::slice::from_ref(&src), &out, JobOptions { delete_originals: true, ..opts() });
    assert_eq!(r.outcome, Outcome::CompletedWithIssues);
    assert_eq!(archive_entries(&out), want);
    assert_eq!(remaining_files(&src), vec![PathBuf::from("readme.txt")]);
    assert_eq!(r.kept.len(), 1);
    assert!(r.kept[0].1.contains("read-only"));
}

#[test]
fn refuses_to_overwrite_an_existing_archive() {
    let (_t, src, out) = setup();
    fs::write(&out, b"existing").unwrap();
    assert!(matches!(create(std::slice::from_ref(&src), &out, opts()), Err(Error::OutputExists(_))));
    assert_eq!(fs::read(&out).unwrap(), b"existing");
    let r = create_and_run(std::slice::from_ref(&src), &out, JobOptions { overwrite: true, ..opts() });
    assert_eq!(r.outcome, Outcome::Completed);
    assert_eq!(archive_entries(&out), expected_entries(&src));
}

#[test]
fn refuses_output_inside_a_source_and_overlapping_sources() {
    let (_t, src, _) = setup();
    let inside = src.join("x.zip");
    assert!(matches!(create(std::slice::from_ref(&src), &inside, opts()), Err(Error::Invalid(_))));
    assert!(matches!(create(&[src.clone(), src.join("a")], &src.parent().unwrap().join("o.zip"), opts()), Err(Error::Invalid(_))));
}

#[test]
fn refuses_to_start_over_an_unfinished_job() {
    let (_t, src, out) = setup();
    let job = create(std::slice::from_ref(&src), &out, opts()).unwrap();
    drop(job);
    assert!(matches!(create(std::slice::from_ref(&src), &out, opts()), Err(Error::JobExists { .. })));
    // ...and resuming it works.
    let r = resume(&out);
    assert_eq!(r.outcome, Outcome::Completed);
    assert_eq!(archive_entries(&out), expected_entries(&src));
}

#[test]
fn a_second_process_cannot_run_the_same_job() {
    let (_t, src, out) = setup();
    let _job = create(std::slice::from_ref(&src), &out, opts()).unwrap();
    assert!(matches!(Job::open(&out), Err(Error::Busy(_))));
}

#[test]
fn cancel_then_resume_produces_identical_archive() {
    let (_t, src, out) = setup();
    let want = expected_entries(&src);
    let mut job = create(std::slice::from_ref(&src), &out, JobOptions { delete_originals: true, batch_entries: 1, ..opts() }).unwrap();
    let cancel = Arc::new(AtomicBool::new(false));
    let c2 = cancel.clone();
    let mut seen = 0;
    let mut obs = move |e: Event| {
        if let Event::Entry { status: zipshrink_core::EntryStatus::Deleted, .. } = e {
            seen += 1;
            if seen == 3 {
                c2.store(true, Ordering::SeqCst);
            }
        }
    };
    let r = job.run(&mut obs, &cancel);
    assert!(matches!(r.outcome, Outcome::Stopped { kind: StopKind::Cancelled, resumable: true, .. }), "{r:?}");
    assert!(r.recoverable_entries > 0);
    drop(job);
    let r = resume(&out);
    assert_eq!(r.outcome, Outcome::Completed, "{r:?}");
    assert_eq!(archive_entries(&out), want);
    assert!(!src.exists());
}

#[test]
fn low_space_reserve_pauses_safely_and_resume_finishes() {
    let (_t, src, out) = setup();
    let want = expected_entries(&src);
    let free = zipshrink_core::platform::available_space(out.parent().unwrap()).unwrap();
    let mut job =
        create(std::slice::from_ref(&src), &out, JobOptions { delete_originals: true, min_free: free + (1 << 30), ..opts() }).unwrap();
    let r = run(&mut job);
    assert!(matches!(r.outcome, Outcome::Stopped { kind: StopKind::LowSpace, .. }), "{r:?}");
    assert_eq!(expected_entries(&src), want, "nothing deleted");
    drop(job);
    let mut job = Job::open(&out).unwrap();
    job.set_min_free(16 << 20);
    let r = run(&mut job);
    assert_eq!(r.outcome, Outcome::Completed);
    assert_eq!(archive_entries(&out), want);
}

#[test]
fn preflight_reports_fit_and_misfit() {
    let (_t, src, out) = setup();
    let roots = scan::canonical_roots(std::slice::from_ref(&src)).unwrap();
    let s = scan::scan(&roots, &[], &AtomicBool::new(false)).unwrap();
    let paths = zipshrink_core::JobPaths::for_output(&out);
    let p = space::preflight(&s, &paths, &opts()).unwrap();
    assert_eq!(p.verdict, space::Verdict::Fits, "{}", p.explanation);
    assert!(p.estimated_archive < p.worst_case_archive);
    assert!(p.sampled_ratio.unwrap() < 1.0);
    let huge = JobOptions { min_free: p.free_space, ..opts() };
    let p = space::preflight(&s, &paths, &huge).unwrap();
    assert_eq!(p.verdict, space::Verdict::DoesNotFit);
}

#[test]
fn verify_detects_a_flipped_byte() {
    let (_t, src, out) = setup();
    create_and_run(std::slice::from_ref(&src), &out, JobOptions { level: 0, ..opts() });
    let ok = verify::check_archive(&out, true, &AtomicBool::new(false), &mut |_| {}).unwrap();
    assert!(ok.ok());
    let mut data = fs::read(&out).unwrap();
    let needle = b"lorem ipsum dolor";
    let at = data.windows(needle.len()).position(|w| w == needle).unwrap();
    data[at] ^= 0x20;
    fs::write(&out, &data).unwrap();
    let bad = verify::check_archive(&out, true, &AtomicBool::new(false), &mut |_| {}).unwrap();
    assert!(!bad.ok());
}

#[test]
fn salvage_rebuilds_an_archive_without_its_journal() {
    let (_t, src, out) = setup();
    let want = expected_entries(&src);
    let mut job = create(std::slice::from_ref(&src), &out, JobOptions { batch_entries: 2, ..opts() }).unwrap();
    let cancel = AtomicBool::new(false);
    let mut seen = 0;
    let cref = &cancel;
    let mut obs = |e: Event| {
        if let Event::Entry { status: zipshrink_core::EntryStatus::Archived, .. } = e {
            seen += 1;
            if seen == 6 {
                cref.store(true, Ordering::SeqCst);
            }
        }
    };
    // `cancel` is only read between entries, so the run stops cleanly.
    let r = job.run(&mut obs, cref);
    assert!(matches!(r.outcome, Outcome::Stopped { .. }));
    let paths = job.paths.clone();
    drop(job);
    // Add a torn tail, as a crash in the middle of an entry would.
    let mut f = fs::OpenOptions::new().append(true).open(&paths.partial).unwrap();
    std::io::Write::write_all(&mut f, b"PK\x03\x04garbage").unwrap();
    drop(f);
    fs::remove_file(&paths.journal).unwrap();
    assert!(matches!(Job::open(&out), Err(Error::Damaged(_))));
    let target = zipshrink_core::salvage::default_output(&paths.partial);
    assert_eq!(target, out);
    let rep = zipshrink_core::salvage::salvage(&paths.partial, &target, &AtomicBool::new(false), &mut |_, _| {}).unwrap();
    assert!(rep.entries >= 6 && rep.check.ok());
    let got = archive_entries(&target);
    for (k, v) in &got {
        assert_eq!(want.get(k), Some(v), "{k}");
    }
    assert_eq!(rep.damaged.len(), 1);
}

#[test]
fn journal_and_partial_are_kept_after_failure_and_plan_is_reported() {
    let (_t, src, out) = setup();
    let job = create(std::slice::from_ref(&src), &out, opts()).unwrap();
    assert!(job.paths.partial.exists() && job.paths.journal.exists());
    assert!(!job.plan().is_empty());
    let _ = Silent;
}

#[test]
fn split_into_parts_that_each_open_and_together_rebuild_the_tree() {
    use zipshrink_core::split;
    let (_t, src, out) = setup();
    let want = expected_entries(&src);
    let roots = scan::canonical_roots(std::slice::from_ref(&src)).unwrap();
    let s = scan::scan(&roots, &[], &AtomicBool::new(false)).unwrap();
    let ratio = space::sample_ratio(&s, 6);
    let parts = split::split_scan(&s, &out, 3, ratio).unwrap();
    assert_eq!(parts.len(), 3);
    assert_eq!(parts.iter().map(|p| p.bytes).sum::<u64>(), s.bytes);
    let req = zipshrink_core::CreateRequest {
        sources: vec![src.clone()],
        output: out.clone(),
        options: JobOptions { delete_originals: true, ..opts() },
    };
    let mut jobs = split::create_jobs(&req, &parts).unwrap();
    // Interrupt the set: run only the first part, then resume the rest by name.
    let r = run(&mut jobs[0]);
    assert_eq!(r.outcome, Outcome::Completed, "{r:?}");
    drop(jobs);
    let pending = split::unfinished_parts(&out);
    assert_eq!(pending, vec![parts[1].output.clone(), parts[2].output.clone()]);
    assert_eq!(split::unfinished_parts(&parts[2].output), pending);
    for p in pending {
        let r = resume(&p);
        assert!(r.finished(), "{r:?}");
    }
    let mut union = std::collections::BTreeMap::new();
    for p in &parts {
        let e = archive_entries(&p.output);
        assert!(e.values().any(|v| v != "dir"), "every part holds files");
        for (k, v) in e {
            if let Some(old) = union.insert(k.clone(), v.clone()) {
                assert_eq!(old, "dir", "{k} is in two parts");
            }
        }
    }
    assert_eq!(union, want);
    assert!(!src.exists(), "the emptied tree is removed by the last part");
}

/// A real file over 4 GiB, so ZIP64 sizes are required. Slow; run with `--ignored`.
#[test]
#[ignore]
fn zip64_file_larger_than_4_gib() {
    use std::io::Write;
    let t = tempfile::tempdir().unwrap();
    let src = t.path().join("huge");
    fs::create_dir_all(&src).unwrap();
    let mut f = fs::File::create(src.join("disk.img")).unwrap();
    let block = noise(5, 1 << 20);
    let zero = vec![0u8; 1 << 20];
    for i in 0..(4u64 * 1024 + 300) {
        f.write_all(if i % 512 == 0 { &block } else { &zero }).unwrap();
    }
    drop(f);
    let want = expected_entries(&src);
    let out = t.path().join("huge.zip");
    let r = create_and_run(&[src.clone()], &out, JobOptions { level: 1, delete_originals: true, ..opts() });
    assert_eq!(r.outcome, Outcome::Completed, "{r:?}");
    assert!(r.original_bytes > 0xffff_ffff);
    assert_eq!(archive_entries(&out), want);
    assert!(!src.exists());
}

/// More than 65,535 entries, so the ZIP64 end of central directory is required.
#[test]
#[ignore]
fn zip64_more_than_65535_entries() {
    let t = tempfile::tempdir().unwrap();
    let src = t.path().join("many");
    for d in 0..70 {
        let dir = src.join(format!("d{d:02}"));
        fs::create_dir_all(&dir).unwrap();
        for i in 0..1000 {
            fs::write(dir.join(format!("{i}.txt")), format!("{d}-{i}")).unwrap();
        }
    }
    let want = expected_entries(&src);
    assert!(want.len() > 65_535);
    let out = t.path().join("many.zip");
    let r = create_and_run(&[src.clone()], &out, JobOptions { threads: 4, ..opts() });
    assert_eq!(r.outcome, Outcome::Completed, "{r:?}");
    assert_eq!(archive_entries(&out), want);
}
