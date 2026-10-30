//! Disk-space preflight: will the job fit, and why.

use crate::engine::JobPaths;
use crate::error::{Error, Result};
use crate::format;
use crate::journal::JobOptions;
use crate::platform;
use crate::scan::ScanResult;
use crate::util::human_bytes;
use std::io::Read;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Fits even if nothing compresses.
    Fits,
    /// Fits at the sampled compression ratio, not in the worst case. The
    /// job pauses safely if space runs low.
    ProbablyFits,
    DoesNotFit,
}

#[derive(Debug, Clone)]
pub struct Preflight {
    pub files: u64,
    pub dirs: u64,
    pub bytes: u64,
    pub largest: u64,
    pub skipped: Vec<(PathBuf, String)>,
    pub warnings: Vec<String>,
    /// Archive size at the sampled compression ratio.
    pub estimated_archive: u64,
    /// Archive size if nothing compresses.
    pub worst_case_archive: u64,
    pub sampled_ratio: Option<f64>,
    pub dest_dir: PathBuf,
    pub free_space: u64,
    pub min_free: u64,
    /// Bytes of selected data on the destination volume (only these give
    /// space back when deleted).
    pub same_volume_bytes: u64,
    pub delete_originals: bool,
    /// Peak extra space the job needs, worst case and estimated.
    pub needed_worst: u64,
    pub needed_estimate: u64,
    pub fs_name: Option<String>,
    pub verdict: Verdict,
    pub explanation: String,
}

const BATCH_SLACK: u64 = 64 << 20;

fn overhead(name_bytes: u64, entries: u64) -> u64 {
    // Local + central header with our extra fields, per entry, plus the end records.
    2 * name_bytes + entries * 230 + 200
}

/// Compresses samples of the selection to estimate the ratio.
pub fn sample_ratio(scan: &ScanResult, level: u32) -> Option<f64> {
    if level == 0 {
        return Some(1.0);
    }
    let mut files: Vec<_> = scan.entries.iter().filter(|e| !e.meta.is_dir && e.size > 0).collect();
    if files.is_empty() {
        return None;
    }
    files.sort_by_key(|e| std::cmp::Reverse(e.size));
    let mut picks: Vec<_> = files.iter().take(12).copied().collect();
    let step = (files.len() / 12).max(1);
    picks.extend(files.iter().step_by(step).take(12).copied());
    picks.dedup_by_key(|e| e.idx);
    let (mut weight, mut weighted) = (0f64, 0f64);
    let mut buf = vec![0u8; 512 << 10];
    for e in picks {
        let Ok((f, _)) = platform::open_source(&e.src) else { continue };
        let mut f = f;
        let mut n = 0;
        while n < buf.len() {
            match f.read(&mut buf[n..]) {
                Ok(0) => break,
                Ok(k) => n += k,
                Err(_) => break,
            }
        }
        if n == 0 {
            continue;
        }
        let mut z = flate2::Compress::new(flate2::Compression::new(level), false);
        let mut out = Vec::with_capacity(n + 1024);
        if z.compress_vec(&buf[..n], &mut out, flate2::FlushCompress::Finish).is_err() {
            continue;
        }
        let r = (out.len() as f64 / n as f64).min(1.0);
        let w = e.size as f64;
        weight += w;
        weighted += r * w;
    }
    (weight > 0.0).then(|| weighted / weight)
}

pub fn preflight(scan: &ScanResult, paths: &JobPaths, opts: &JobOptions) -> Result<Preflight> {
    let dest_dir = paths.dest_dir();
    let free_space = platform::available_space(&dest_dir).map_err(|e| Error::io(&dest_dir, e))?;
    let min_free = opts.min_free.max(crate::MIN_FREE_FLOOR);
    let dest_vol = platform::volume_id(&dest_dir).ok();
    let mut same_volume_bytes = 0;
    let mut same_volume_largest = 0;
    let mut other_bytes = 0;
    let root_vols: Vec<Option<u64>> = scan
        .roots
        .iter()
        .map(|r| {
            let dir = if r.is_dir() { r.clone() } else { r.parent().map(|p| p.to_path_buf()).unwrap_or_default() };
            platform::volume_id(&dir).ok()
        })
        .collect();
    let mut name_bytes = 0u64;
    for e in &scan.entries {
        name_bytes += e.meta.name.len() as u64;
        if e.meta.is_dir {
            continue;
        }
        let same = dest_vol.is_some() && root_vols[e.root as usize] == dest_vol;
        if same {
            same_volume_bytes += e.size;
            same_volume_largest = same_volume_largest.max(e.size);
        } else {
            other_bytes += e.size;
        }
    }
    let entries = scan.entries.len() as u64;
    let ovh = overhead(name_bytes, entries);
    let ratio = sample_ratio(scan, opts.level);
    let est = |n: u64| (n as f64 * ratio.unwrap_or(1.0)).ceil() as u64;
    let worst_case_archive = format::worst_case_compressed(scan.bytes) + ovh;
    let estimated_archive = est(scan.bytes) + ovh;
    let (needed_worst, needed_estimate) = if opts.delete_originals {
        // Data from the destination volume is freed as it is archived, so
        // only about one batch (or the largest file) is ever held twice.
        let peak =
            format::worst_case_compressed(same_volume_largest).max(BATCH_SLACK.min(format::worst_case_compressed(same_volume_bytes)));
        let peak_est = est(same_volume_largest).max(BATCH_SLACK.min(est(same_volume_bytes)));
        (format::worst_case_compressed(other_bytes) + peak + ovh, est(other_bytes) + peak_est + ovh)
    } else {
        (worst_case_archive, estimated_archive)
    };
    let usable = free_space.saturating_sub(min_free);
    let verdict = if usable >= needed_worst {
        Verdict::Fits
    } else if usable >= needed_estimate {
        Verdict::ProbablyFits
    } else {
        Verdict::DoesNotFit
    };
    let mut warnings = scan.warnings.clone();
    let fs_name = platform::fs_name(&dest_dir);
    if let Some(fs) = &fs_name {
        if fs.eq_ignore_ascii_case("FAT") || fs.eq_ignore_ascii_case("FAT32") {
            warnings.push("the destination is FAT32, which cannot hold files of 4 GiB or more".into());
        }
    }
    if opts.delete_originals && same_volume_bytes == 0 && scan.bytes > 0 {
        warnings.push(
            "the selected files are on a different drive from the archive, so deleting them does not free space on the destination".into(),
        );
    }
    let explanation = match verdict {
        Verdict::Fits => format!(
            "It fits: the job needs at most {} of the {} available (keeping {} free).",
            human_bytes(needed_worst),
            human_bytes(free_space),
            human_bytes(min_free)
        ),
        Verdict::ProbablyFits => format!(
            "It should fit: about {} is needed at the sampled compression ratio, but up to {} if the data does not compress; {} is available (keeping {} free). If space runs low the job pauses safely and can be resumed.",
            human_bytes(needed_estimate),
            human_bytes(needed_worst),
            human_bytes(free_space),
            human_bytes(min_free)
        ),
        Verdict::DoesNotFit => format!(
            "It does not fit: about {} is needed but only {} is available (keeping {} free).{}",
            human_bytes(needed_estimate),
            human_bytes(free_space),
            human_bytes(min_free),
            if opts.delete_originals { "" } else { " Enabling deletion of originals, or choosing another destination, would reduce this." }
        ),
    };
    Ok(Preflight {
        files: scan.files,
        dirs: scan.dirs,
        bytes: scan.bytes,
        largest: scan.largest,
        skipped: scan.skipped.clone(),
        warnings,
        estimated_archive,
        worst_case_archive,
        sampled_ratio: ratio,
        dest_dir,
        free_space,
        min_free,
        same_volume_bytes,
        delete_originals: opts.delete_originals,
        needed_worst,
        needed_estimate,
        fs_name,
        verdict,
        explanation,
    })
}
