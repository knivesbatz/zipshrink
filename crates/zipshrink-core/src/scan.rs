//! Planning: turns the user's selection into an ordered list of entries.
//!
//! Links of every kind are recorded as skipped and never followed, so
//! nothing outside the selected folders is read, archived or deleted.

use crate::error::{Error, Result};
use crate::format::EntryMeta;
use crate::journal::PlanEntry;
use crate::platform::{self, FileStat, Kind};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Debug, Default, Clone)]
pub struct ScanResult {
    pub roots: Vec<PathBuf>,
    pub entries: Vec<PlanEntry>,
    pub skipped: Vec<(PathBuf, String)>,
    pub warnings: Vec<String>,
    pub files: u64,
    pub dirs: u64,
    pub bytes: u64,
    pub largest: u64,
}

fn same_path(a: &Path, b: &Path) -> bool {
    if cfg!(any(windows, target_os = "macos")) {
        a.as_os_str().to_string_lossy().to_lowercase() == b.as_os_str().to_string_lossy().to_lowercase()
    } else {
        a == b
    }
}

/// Whether `p` is `root` or lies inside it (case-insensitively on Windows and macOS).
pub fn is_within(p: &Path, root: &Path) -> bool {
    let mut pc = p.components();
    for rc in root.components() {
        match pc.next() {
            Some(c) if same_path(Path::new(c.as_os_str()), Path::new(rc.as_os_str())) => {}
            _ => return false,
        }
    }
    true
}

/// Validates and canonicalises the selected sources.
pub fn canonical_roots(sources: &[PathBuf]) -> Result<Vec<PathBuf>> {
    if sources.is_empty() {
        return Err(Error::Invalid("no files or folders were selected".into()));
    }
    let mut roots: Vec<PathBuf> = Vec::new();
    for s in sources {
        let (kind, _) = platform::kind_of(s).map_err(|e| Error::io(s, e))?;
        match kind {
            Kind::File | Kind::Dir => {}
            Kind::Link => {
                return Err(Error::Invalid(format!(
                    "{} is a symbolic link or junction; select the folder it points to instead",
                    s.display()
                )))
            }
            Kind::Other(what) => return Err(Error::Invalid(format!("{} is a {what} and cannot be archived", s.display()))),
        }
        let c = dunce::canonicalize(s).map_err(|e| Error::io(s, e))?;
        if c.file_name().is_none() {
            return Err(Error::Invalid(format!("{} is the root of a drive; select folders inside it instead", c.display())));
        }
        for r in &roots {
            if is_within(&c, r) || is_within(r, &c) {
                return Err(Error::Invalid(format!("{} and {} overlap; select each folder only once", r.display(), c.display())));
            }
        }
        roots.push(c);
    }
    Ok(roots)
}

fn meta_for(name: String, is_dir: bool, st: &FileStat) -> EntryMeta {
    EntryMeta {
        name,
        is_dir,
        mtime: st.mtime,
        atime: st.atime,
        btime: st.btime,
        unix_mode: st.unix_mode,
        dos_attrs: st.dos_attrs | if is_dir { 0x10 } else { 0 },
    }
}

fn unique_top_name(used: &mut HashSet<String>, name: &str, is_dir: bool) -> String {
    let key = |s: &str| s.to_lowercase();
    if used.insert(key(name)) {
        return name.to_string();
    }
    let (stem, ext) = match (is_dir, name.rfind('.')) {
        (false, Some(i)) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    for n in 2.. {
        let candidate = format!("{stem} ({n}){ext}");
        if used.insert(key(&candidate)) {
            return candidate;
        }
    }
    unreachable!()
}

/// Walks the roots and builds the plan. `exclude` lists paths never to
/// include (the archive being written and its journal).
pub fn scan(roots: &[PathBuf], exclude: &[PathBuf], cancel: &AtomicBool) -> Result<ScanResult> {
    let mut out = ScanResult { roots: roots.to_vec(), ..Default::default() };
    let mut used = HashSet::new();
    for (ri, root) in roots.iter().enumerate() {
        let base = root.file_name().unwrap().to_str().map(str::to_owned);
        let Some(base) = base else {
            out.skipped.push((root.clone(), "its name is not valid Unicode, so it cannot be stored in a ZIP".into()));
            continue;
        };
        let (kind, m) = platform::kind_of(root).map_err(|e| Error::io(root, e))?;
        let is_dir = kind == Kind::Dir;
        let top = unique_top_name(&mut used, &base, is_dir);
        if top != base {
            out.warnings.push(format!("{} is stored as \"{top}\" because another selected item has the same name", root.display()));
        }
        let st = platform::stat_from_metadata(&m);
        if is_dir {
            walk_dir(&mut out, ri as u32, root, format!("{top}/"), st, exclude, cancel)?;
        } else {
            push_file(&mut out, ri as u32, root.clone(), top, &st);
        }
    }
    let mut case_seen = HashSet::new();
    let mut case_clashes = 0;
    for e in &out.entries {
        if !case_seen.insert(e.meta.name.to_lowercase()) {
            case_clashes += 1;
        }
    }
    if case_clashes > 0 {
        out.warnings.push(format!("{case_clashes} names differ only in letter case; extracting on Windows or macOS would merge them"));
    }
    Ok(out)
}

fn push_file(out: &mut ScanResult, root: u32, src: PathBuf, name: String, st: &FileStat) {
    if name.len() > u16::MAX as usize {
        out.skipped.push((src, "its path is too long to store in a ZIP".into()));
        return;
    }
    out.files += 1;
    out.bytes += st.size;
    out.largest = out.largest.max(st.size);
    out.entries.push(PlanEntry {
        idx: out.entries.len() as u64,
        src,
        root,
        meta: meta_for(name, false, st),
        size: st.size,
        dir_stat: None,
    });
}

fn walk_dir(
    out: &mut ScanResult,
    root: u32,
    dir: &Path,
    name: String,
    st: FileStat,
    exclude: &[PathBuf],
    cancel: &AtomicBool,
) -> Result<()> {
    // Explicit stack: deep trees must not overflow the thread stack.
    let mut stack: Vec<(PathBuf, String, FileStat)> = vec![(dir.to_path_buf(), name, st)];
    while let Some((dir, name, st)) = stack.pop() {
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        if name.len() > u16::MAX as usize {
            out.skipped.push((dir, "its path is too long to store in a ZIP".into()));
            continue;
        }
        out.dirs += 1;
        out.entries.push(PlanEntry {
            idx: out.entries.len() as u64,
            src: dir.clone(),
            root,
            meta: meta_for(name.clone(), true, &st),
            size: 0,
            dir_stat: Some(st),
        });
        let rd = match std::fs::read_dir(&dir) {
            Ok(rd) => rd,
            Err(e) => {
                out.skipped.push((dir.clone(), format!("the folder could not be read: {e}")));
                continue;
            }
        };
        let mut children = Vec::new();
        for ent in rd {
            match ent {
                Ok(ent) => children.push(ent.path()),
                Err(e) => out.skipped.push((dir.clone(), format!("part of the folder could not be read: {e}"))),
            }
        }
        children.sort();
        let mut subdirs = Vec::new();
        for child in children {
            if exclude.iter().any(|x| same_path(x, &child)) {
                continue;
            }
            let Some(cname) = child.file_name().and_then(|n| n.to_str()).map(str::to_owned) else {
                out.skipped.push((child, "its name is not valid Unicode, so it cannot be stored in a ZIP".into()));
                continue;
            };
            let (kind, m) = match platform::kind_of(&child) {
                Ok(x) => x,
                Err(e) => {
                    out.skipped.push((child, format!("could not be examined: {e}")));
                    continue;
                }
            };
            let cst = platform::stat_from_metadata(&m);
            match kind {
                Kind::File => push_file(out, root, child, format!("{name}{cname}"), &cst),
                Kind::Dir => subdirs.push((child, format!("{name}{cname}/"), cst)),
                Kind::Link => out.skipped.push((child, "symbolic link, junction or reparse point (not followed, not deleted)".into())),
                Kind::Other(what) => out.skipped.push((child, format!("{what} (not archived, not deleted)"))),
            }
        }
        // Reverse so the first subdirectory is processed next: files of a
        // folder come before its subfolders, subfolders in name order.
        stack.extend(subdirs.into_iter().rev());
    }
    Ok(())
}
