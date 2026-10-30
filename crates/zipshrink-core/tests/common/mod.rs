#![allow(dead_code)]

use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use zipshrink_core::progress::Silent;
use zipshrink_core::{CreateRequest, Job, JobOptions, Report};

/// Deterministic pseudo-random bytes (xorshift), incompressible.
pub fn noise(seed: u64, len: usize) -> Vec<u8> {
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    let mut v = Vec::with_capacity(len);
    while v.len() < len {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        v.extend_from_slice(&x.to_le_bytes());
    }
    v.truncate(len);
    v
}

pub fn write(p: &Path, data: &[u8]) {
    fs::create_dir_all(p.parent().unwrap()).unwrap_or_else(|e| panic!("mkdir {}: {e}", p.parent().unwrap().display()));
    fs::write(p, data).unwrap_or_else(|e| panic!("writing {}: {e}", p.display()));
}

/// A tree with the awkward cases: empty files and folders, Unicode,
/// duplicate names in different folders, compressible and random data.
pub fn sample_tree(root: &Path) {
    write(&root.join("readme.txt"), b"hello, world\n");
    write(&root.join("empty.bin"), b"");
    write(&root.join("text/lorem.txt"), "lorem ipsum dolor sit amet ".repeat(20_000).as_bytes());
    write(&root.join("random/a.bin"), &noise(1, 3 << 20));
    write(&root.join("random/b.bin"), &noise(2, 70_000));
    write(&root.join("a/same.txt"), b"first");
    write(&root.join("b/same.txt"), b"second");
    write(&root.join("ünïcödé/日本語のファイル.txt"), "テキスト".as_bytes());
    // Wine cannot create names outside the Basic Multilingual Plane; real
    // Windows, Linux and macOS can.
    let emoji = root.join("ünïcödé/emoji 🎮 save.dat");
    if fs::write(&emoji, noise(3, 1000)).is_err() {
        eprintln!("note: this filesystem rejected {}; skipping it", emoji.display());
    }
    write(&root.join("deep/1/2/3/4/5/6/7/8/leaf.txt"), b"deep");
    fs::create_dir_all(root.join("empty dir/nested empty")).unwrap();
}

pub fn sha(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
}

/// Archive name -> SHA-256 (files) or "dir" for every item under `root`,
/// named as ZipShrink names them (`<root name>/...`).
pub fn expected_entries(root: &Path) -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    let base = root.file_name().unwrap().to_str().unwrap().to_string();
    fn walk(p: &Path, name: String, m: &mut BTreeMap<String, String>) {
        let md = fs::symlink_metadata(p).unwrap();
        if md.file_type().is_symlink() {
            return;
        }
        if md.is_dir() {
            m.insert(format!("{name}/"), "dir".into());
            for e in fs::read_dir(p).unwrap() {
                let e = e.unwrap();
                walk(&e.path(), format!("{name}/{}", e.file_name().to_str().unwrap()), m);
            }
        } else {
            m.insert(name, sha(&fs::read(p).unwrap()));
        }
    }
    walk(root, base, &mut m);
    m
}

/// Extracts every entry with the independent `zip` reader and hashes it.
pub fn archive_entries(zip_path: &Path) -> BTreeMap<String, String> {
    let mut z = zip::ZipArchive::new(fs::File::open(zip_path).unwrap()).unwrap();
    let mut m = BTreeMap::new();
    for i in 0..z.len() {
        let mut f = z.by_index(i).unwrap();
        let name = String::from_utf8(f.name_raw().to_vec()).unwrap();
        if f.is_dir() {
            m.insert(name, "dir".into());
        } else {
            let mut data = Vec::new();
            f.read_to_end(&mut data).unwrap();
            assert!(m.insert(name.clone(), sha(&data)).is_none(), "duplicate entry {name}");
        }
    }
    m
}

pub fn opts() -> JobOptions {
    JobOptions { min_free: 16 << 20, ..Default::default() }
}

pub fn create(sources: &[PathBuf], output: &Path, options: JobOptions) -> zipshrink_core::Result<Job> {
    let req = CreateRequest { sources: sources.to_vec(), output: output.to_path_buf(), options };
    Job::create(&req, &mut Silent, &AtomicBool::new(false))
}

pub fn run(job: &mut Job) -> Report {
    job.run(&mut Silent, &AtomicBool::new(false))
}

pub fn create_and_run(sources: &[PathBuf], output: &Path, options: JobOptions) -> Report {
    let mut job = create(sources, output, options).expect("job created");
    run(&mut job)
}

pub fn resume(output: &Path) -> Report {
    let mut job = Job::open(output).expect("job opens for resume");
    run(&mut job)
}

/// Files still present under `root` (relative paths).
pub fn remaining_files(root: &Path) -> Vec<PathBuf> {
    let mut v = Vec::new();
    if !root.exists() {
        return v;
    }
    fn walk(p: &Path, base: &Path, v: &mut Vec<PathBuf>) {
        for e in fs::read_dir(p).unwrap() {
            let e = e.unwrap();
            let t = e.file_type().unwrap();
            if t.is_dir() {
                walk(&e.path(), base, v);
            } else {
                v.push(e.path().strip_prefix(base).unwrap().to_path_buf());
            }
        }
    }
    walk(root, root, &mut v);
    v.sort();
    v
}

/// Name -> SHA-256 of every entry committed in the journal of `output`'s job.
pub fn committed_hashes(output: &Path) -> BTreeMap<String, String> {
    use zipshrink_core::journal::{load, Record};
    let paths = zipshrink_core::JobPaths::for_output(output);
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
