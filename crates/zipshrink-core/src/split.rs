//! Splitting one selection into several independent ZIP archives.
//!
//! Each part is a complete, standard ZIP that opens on its own (not a
//! spanned `.z01` volume set). Files are assigned in archive order and a
//! single file is never cut in two. A selection is split either into a
//! number of parts holding about the same amount of data ([`split_scan`]),
//! or into parts of at most a chosen size ([`split_scan_size`]). Every part repeats the folder
//! entries above its files, so extracting all parts into one folder
//! rebuilds the original tree. Each part is its own job with its own
//! journal, so each one is crash-safe and resumable.

use crate::engine::JobPaths;
use crate::error::{Error, Result};
use crate::scan::ScanResult;
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

pub const MAX_PARTS: usize = 999;

#[derive(Debug, Clone)]
pub struct PartPlan {
    /// 1-based part number.
    pub number: usize,
    pub output: PathBuf,
    pub scan: ScanResult,
    pub files: u64,
    pub bytes: u64,
    /// Expected archive size at the sampled compression ratio.
    pub estimated: u64,
    /// Archive size if nothing compresses.
    pub worst_case: u64,
}

/// `Games.zip`, part 2 of 3 -> `Games.part2.zip`; with 10 or more parts
/// the number is zero-padded (`Games.part02.zip`) so names sort correctly.
pub fn part_path(output: &Path, number: usize, parts: usize) -> PathBuf {
    let name = output.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name.as_str(), ""),
    };
    let width = parts.to_string().len();
    output.with_file_name(format!("{stem}.part{number:0width$}{ext}"))
}

/// Smallest size accepted by [`split_scan_size`].
pub const MIN_PART_SIZE: u64 = 64 * 1024;

/// Bytes every archive needs besides its entries (end records).
const PART_BASE_OVERHEAD: u64 = 200;

/// Headers and central-directory record for one entry (local and central
/// copies of the name, fixed fields, ZIP64 and timestamp extras).
fn entry_overhead(name: &str) -> u64 {
    2 * name.len() as u64 + 230
}

/// Divides `scan` into `parts` balanced parts.
pub fn split_scan(scan: &ScanResult, output: &Path, parts: usize, ratio: Option<f64>) -> Result<Vec<PartPlan>> {
    if parts < 2 {
        return Err(Error::Invalid("a split needs at least 2 parts".into()));
    }
    if parts > MAX_PARTS {
        return Err(Error::Invalid(format!("at most {MAX_PARTS} parts are supported")));
    }
    let files = scan.entries.iter().filter(|e| !e.meta.is_dir).count();
    if files < parts {
        return Err(Error::Invalid(format!("the selection has {files} files, so it cannot be split into {parts} parts")));
    }
    // Assign entries in order; part k ends once the running total reaches
    // k/parts of the data, while leaving at least one file per later part.
    let total: u64 = scan.bytes.max(1);
    let mut assign = vec![0usize; scan.entries.len()];
    let mut part = 0usize;
    let mut acc = 0u64;
    let mut files_seen = 0usize;
    let mut files_in_part = 0usize;
    for (i, e) in scan.entries.iter().enumerate() {
        if !e.meta.is_dir {
            let remaining_files = files - files_seen; // including this one
            let boundary = total as u128 * (part as u128 + 1) / parts as u128;
            // Later parts must each still get at least one file.
            let must_cut = remaining_files < parts - part;
            if part + 1 < parts && files_in_part > 0 && (acc as u128 >= boundary || must_cut) {
                part += 1;
                files_in_part = 0;
            }
            acc += e.size;
            files_seen += 1;
            files_in_part += 1;
        }
        assign[i] = part;
    }
    Ok(build_parts(scan, output, parts, assign, ratio))
}

/// Divides `scan` into parts whose estimated archive size (at `ratio`, or
/// stored size when `None`) stays at or below `size` bytes. Files are taken
/// in archive order and a new part starts when the next file would take the
/// current one over the limit; a file larger than `size` gets a part of its
/// own (files are never cut). Returns an empty list when everything fits in
/// one archive of that size, meaning there is nothing to split.
pub fn split_scan_size(scan: &ScanResult, output: &Path, size: u64, ratio: Option<f64>) -> Result<Vec<PartPlan>> {
    if size < MIN_PART_SIZE {
        return Err(Error::Invalid(format!("the size of each ZIP file should be at least {} KB", MIN_PART_SIZE / 1024)));
    }
    let r = ratio.unwrap_or(1.0);
    let mut assign = vec![0usize; scan.entries.len()];
    let mut part = 0usize;
    let mut files_in_part = 0usize;
    let mut cur = PART_BASE_OVERHEAD;
    for (i, e) in scan.entries.iter().enumerate() {
        let mut add = entry_overhead(&e.meta.name);
        if !e.meta.is_dir {
            add += estimate(e.size, r);
            if files_in_part > 0 && cur.saturating_add(add) > size {
                part += 1;
                if part >= MAX_PARTS {
                    return Err(Error::Invalid(format!("this would make more than {MAX_PARTS} ZIP files; choose a larger size")));
                }
                files_in_part = 0;
                cur = PART_BASE_OVERHEAD;
                // The new part repeats the folders above this file.
                let name = e.meta.name.as_str();
                cur += name.match_indices('/').map(|(j, _)| entry_overhead(&name[..=j])).sum::<u64>();
            }
            files_in_part += 1;
        }
        cur = cur.saturating_add(add);
        assign[i] = part;
    }
    if part == 0 {
        return Ok(Vec::new());
    }
    Ok(build_parts(scan, output, part + 1, assign, ratio))
}

fn estimate(bytes: u64, ratio: f64) -> u64 {
    (bytes as f64 * ratio).ceil() as u64
}

/// Builds the parts from a part number per file (folders are filled in
/// here: each goes with the next file after it, and every part also gets
/// the folders above its files).
fn build_parts(scan: &ScanResult, output: &Path, parts: usize, mut assign: Vec<usize>, ratio: Option<f64>) -> Vec<PartPlan> {
    // A folder goes with the next file after it, so a folder whose contents
    // start a new part is not left empty at the end of the previous one.
    let mut next_part = parts - 1;
    for i in (0..scan.entries.len()).rev() {
        if scan.entries[i].meta.is_dir {
            assign[i] = next_part;
        } else {
            next_part = assign[i];
        }
    }
    let by_name: HashMap<&str, usize> =
        scan.entries.iter().enumerate().filter(|(_, e)| e.meta.is_dir).map(|(i, e)| (e.meta.name.as_str(), i)).collect();
    let mut members: Vec<BTreeSet<usize>> = vec![BTreeSet::new(); parts];
    for (i, e) in scan.entries.iter().enumerate() {
        let p = assign[i];
        members[p].insert(i);
        // Parent folders, so each part extracts into the same tree.
        let name = e.meta.name.trim_end_matches('/');
        let mut cut = name.len();
        while let Some(slash) = name[..cut].rfind('/') {
            if let Some(&d) = by_name.get(&name[..=slash]) {
                members[p].insert(d);
            }
            cut = slash;
        }
    }
    let ratio = ratio.unwrap_or(1.0);
    let mut out = Vec::with_capacity(parts);
    for (k, set) in members.into_iter().enumerate() {
        let mut s = ScanResult { roots: scan.roots.clone(), ..Default::default() };
        if k == 0 {
            s.skipped = scan.skipped.clone();
            s.warnings = scan.warnings.clone();
        }
        let mut overhead = PART_BASE_OVERHEAD;
        for i in set {
            let mut e = scan.entries[i].clone();
            e.idx = s.entries.len() as u64;
            overhead += entry_overhead(&e.meta.name);
            if e.meta.is_dir {
                s.dirs += 1;
            } else {
                s.files += 1;
                s.bytes += e.size;
                s.largest = s.largest.max(e.size);
            }
            s.entries.push(e);
        }
        out.push(PartPlan {
            number: k + 1,
            output: part_path(output, k + 1, parts),
            files: s.files,
            bytes: s.bytes,
            estimated: estimate(s.bytes, ratio) + overhead,
            worst_case: crate::format::worst_case_compressed(s.bytes) + overhead,
            scan: s,
        });
    }
    out
}

/// Checks that no part's archive or unfinished job is in the way.
pub fn check_part_paths(parts: &[PartPlan], overwrite: bool) -> Result<()> {
    for p in parts {
        let paths = JobPaths::for_output(&p.output);
        if paths.has_unfinished_job() {
            return Err(Error::JobExists { partial: paths.partial });
        }
        if p.output.exists() && !overwrite {
            return Err(Error::OutputExists(p.output.clone()));
        }
    }
    Ok(())
}

/// Splits `Games.part2.zip` into (`Games`, `.zip`); `None` if not a part name.
fn parse_part_name(name: &str) -> Option<(&str, &str)> {
    let i = name.rfind(".part")?;
    let rest = &name[i + 5..];
    let digits = rest.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits == 0 {
        return None;
    }
    Some((&name[..i], &rest[digits..]))
}

/// Unfinished parts of the split set that `path` belongs to, in order.
/// `path` may be the original archive name or any part, `.partial` or
/// journal. Empty when `path` is not part of a split set with unfinished jobs.
pub fn unfinished_parts(path: &Path) -> Vec<PathBuf> {
    let out = JobPaths::from_any(path).output;
    let Some(dir) = out.parent() else { return vec![] };
    let dir = if dir.as_os_str().is_empty() { Path::new(".") } else { dir };
    let name = out.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let (stem, ext) = match parse_part_name(&name) {
        Some((s, e)) => (s.to_string(), e.to_string()),
        None => match name.rfind('.') {
            Some(i) if i > 0 => (name[..i].to_string(), name[i..].to_string()),
            _ => (name.clone(), String::new()),
        },
    };
    let mut found: Vec<(u64, PathBuf)> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            let Some(base) = n.strip_suffix(".partial.journal") else { continue };
            if let Some((s, x)) = parse_part_name(base) {
                if s == stem && x == ext {
                    let num: u64 = base[s.len() + 5..base.len() - x.len()].parse().unwrap_or(0);
                    found.push((num, dir.join(base)));
                }
            }
        }
    }
    found.sort();
    found.into_iter().map(|(_, p)| p).collect()
}

/// Creates one job per part, after checking that none is in the way. All
/// journals exist before anything is compressed, so an interruption at any
/// point leaves every remaining part resumable.
pub fn create_jobs(req: &crate::CreateRequest, parts: &[PartPlan]) -> Result<Vec<crate::Job>> {
    check_part_paths(parts, req.options.overwrite)?;
    parts
        .iter()
        .map(|p| {
            let r = crate::CreateRequest { sources: req.sources.clone(), output: p.output.clone(), options: req.options.clone() };
            crate::Job::create_from_scan(&r, p.scan.clone())
        })
        .collect()
}

/// What `resume` on `path` should continue: the named job first (if it has
/// one), then any other unfinished parts of its split set.
pub fn resume_targets(path: &Path) -> Vec<PathBuf> {
    let own = JobPaths::from_any(path);
    let mut targets = Vec::new();
    if own.journal.exists() || own.partial.exists() {
        targets.push(own.output.clone());
    }
    for p in unfinished_parts(path) {
        if !targets.contains(&p) {
            targets.push(p);
        }
    }
    if targets.is_empty() {
        targets.push(path.to_path_buf());
    }
    targets
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::EntryMeta;
    use crate::journal::PlanEntry;

    fn scan_of(items: &[(&str, u64)]) -> ScanResult {
        let mut s = ScanResult::default();
        for (n, size) in items {
            let is_dir = n.ends_with('/');
            s.entries.push(PlanEntry {
                idx: s.entries.len() as u64,
                src: PathBuf::from(n),
                root: 0,
                meta: EntryMeta { name: n.to_string(), is_dir, ..Default::default() },
                size: *size,
                dir_stat: None,
            });
            if !is_dir {
                s.files += 1;
                s.bytes += size;
            }
        }
        s
    }

    #[test]
    fn names() {
        assert_eq!(part_path(Path::new("/x/G.zip"), 2, 3), PathBuf::from("/x/G.part2.zip"));
        assert_eq!(part_path(Path::new("/x/G.zip"), 2, 12), PathBuf::from("/x/G.part02.zip"));
        assert_eq!(parse_part_name("G.part02.zip"), Some(("G", ".zip")));
        assert_eq!(parse_part_name("G.zip"), None);
    }

    #[test]
    fn balanced_and_complete_with_parent_folders() {
        let s = scan_of(&[("r/", 0), ("r/a", 10), ("r/b", 10), ("r/d/", 0), ("r/d/c", 10), ("r/d/e", 10), ("r/empty/", 0)]);
        let parts = split_scan(&s, Path::new("o.zip"), 2, Some(0.5)).unwrap();
        let names: Vec<Vec<&str>> = parts.iter().map(|p| p.scan.entries.iter().map(|e| e.meta.name.as_str()).collect()).collect();
        assert_eq!(names[0], vec!["r/", "r/a", "r/b"]);
        assert_eq!(names[1], vec!["r/", "r/d/", "r/d/c", "r/d/e", "r/empty/"]);
        assert_eq!(parts[0].bytes, 20);
        assert_eq!(parts[1].bytes, 20);
        assert!(parts[0].estimated < parts[0].worst_case);
    }

    #[test]
    fn every_part_gets_a_file_even_when_one_is_huge() {
        let s = scan_of(&[("r/", 0), ("r/huge", 1000), ("r/a", 1), ("r/b", 1)]);
        let parts = split_scan(&s, Path::new("o.zip"), 3, None).unwrap();
        assert!(parts.iter().all(|p| p.files == 1), "{:?}", parts.iter().map(|p| p.files).collect::<Vec<_>>());
        assert!(split_scan(&s, Path::new("o.zip"), 4, None).is_err());
    }

    fn names_of(p: &PartPlan) -> Vec<&str> {
        p.scan.entries.iter().map(|e| e.meta.name.as_str()).collect()
    }

    #[test]
    fn by_size_fills_parts_in_order() {
        let k = 100 * 1024;
        let s = scan_of(&[("r/", 0), ("r/a", k), ("r/b", k), ("r/d/", 0), ("r/d/c", k), ("r/d/e", k), ("r/empty/", 0)]);
        // Two files fit in 250 KB, three do not.
        let parts = split_scan_size(&s, Path::new("o.zip"), 250 * 1024, None).unwrap();
        assert_eq!(parts.len(), 2);
        assert_eq!(names_of(&parts[0]), vec!["r/", "r/a", "r/b"]);
        assert_eq!(names_of(&parts[1]), vec!["r/", "r/d/", "r/d/c", "r/d/e", "r/empty/"]);
        assert_eq!(parts[1].output, PathBuf::from("o.part2.zip"));
        assert!(parts.iter().all(|p| p.estimated <= 250 * 1024), "{:?}", parts.iter().map(|p| p.estimated).collect::<Vec<_>>());
        // At half the size each, three files fit in a part.
        let parts = split_scan_size(&s, Path::new("o.zip"), 160 * 1024, Some(0.5)).unwrap();
        assert_eq!(parts.iter().map(|p| p.files).collect::<Vec<_>>(), vec![3, 1]);
        assert_eq!(parts.iter().map(|p| p.bytes).sum::<u64>(), s.bytes);
    }

    #[test]
    fn by_size_returns_nothing_when_it_all_fits() {
        let s = scan_of(&[("r/", 0), ("r/a", 1000), ("r/b", 1000)]);
        assert!(split_scan_size(&s, Path::new("o.zip"), MIN_PART_SIZE, None).unwrap().is_empty());
        assert!(split_scan_size(&ScanResult::default(), Path::new("o.zip"), MIN_PART_SIZE, None).unwrap().is_empty());
    }

    #[test]
    fn by_size_gives_a_big_file_its_own_part() {
        let s = scan_of(&[("a", 10), ("huge", 10 * MIN_PART_SIZE), ("b", 10), ("c", 10)]);
        let parts = split_scan_size(&s, Path::new("o.zip"), MIN_PART_SIZE, None).unwrap();
        let names: Vec<Vec<&str>> = parts.iter().map(names_of).collect();
        assert_eq!(names, vec![vec!["a"], vec!["huge"], vec!["b", "c"]]);
        assert!(parts[1].estimated > MIN_PART_SIZE);
    }

    #[test]
    fn by_size_limits() {
        let s = scan_of(&[("a", 1), ("b", 1)]);
        assert!(split_scan_size(&s, Path::new("o.zip"), MIN_PART_SIZE - 1, None).is_err());
        let many: Vec<(String, u64)> = (0..=MAX_PARTS).map(|i| (format!("f{i}"), MIN_PART_SIZE)).collect();
        let refs: Vec<(&str, u64)> = many.iter().map(|(n, s)| (n.as_str(), *s)).collect();
        let s = scan_of(&refs);
        let err = split_scan_size(&s, Path::new("o.zip"), MIN_PART_SIZE, None).unwrap_err();
        assert!(err.to_string().contains("more than 999"), "{err}");
        let fine = split_scan_size(&s, Path::new("o.zip"), 2 * MIN_PART_SIZE + 1000, None).unwrap();
        assert_eq!(fine.len(), 500);
        assert_eq!(fine[0].output, PathBuf::from("o.part001.zip"));
    }
}
