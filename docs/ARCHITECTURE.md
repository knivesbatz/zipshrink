# Architecture

```
crates/
  zipshrink-core     engine: planning, ZIP writer, journal, verification, recovery
  zipshrink-cli      `zipshrink` command line tool
  zipshrink-gui      `zipshrink-gui` desktop app (egui/eframe)
  zipshrink-update   update check and verified installer download
packaging/           Windows installer (NSIS), MSI (WiX/wixl), build script
scripts/             font, icon and license generators
```

## Core engine (`zipshrink-core`)

| Module | Responsibility |
|---|---|
| `scan` | Validates and canonicalises the selection (no links, no overlapping roots, no drive roots, archive not inside a source), walks it without following links, and produces the ordered plan. |
| `space` | Preflight: sampled compression ratio, worst case, same-volume analysis, peak need, verdict. |
| `split` | Divides a plan into N balanced, independent parts; names them; finds unfinished parts of a set. |
| `format` | Hand-written ZIP/ZIP64 records: local headers (patched in place), central directory, end records, extra fields; parsers for local headers and central directories. |
| `journal` | Append-only checksummed JSON-lines recovery journal with torn-tail detection and an exclusive lock. |
| `engine` | The `Job`: create, run, batch commit, deletion, finalization, resume, report. |
| `verify` | Read-back check of one entry (header bytes, parse, inflate, CRC-32, SHA-256) and of a whole archive with the independent `zip` crate. |
| `salvage` | Rebuilds a readable archive from a partial archive without its journal. |
| `platform` | OS specifics: no-follow opens, Windows share modes, file identity, flush, uncached reads, verified deletion, no-replace rename, volume ids, free space. |
| `failpoint` | Named fault-injection points, compiled only with the `failpoints` feature. |
| `progress` | Events and statistics for the UIs. |

### Why a custom ZIP writer

Crash recovery needs to know exactly where each entry begins and ends, and
needs entries that can be found again without a central directory. Every
entry is written as a local header followed by its data — **no data
descriptors** — and the header is rewritten in place once the CRC-32 and
sizes are known (its length never changes: the ZIP64 extra field is reserved
up front for files that could exceed 4 GiB, and the SHA-256 extra field is
reserved with zeros). The partial archive is therefore always a sequence of
self-describing entries, which is what both `resume` and `salvage` rely on.

General-purpose ZIP writers buffer or own these decisions; the `zip` crate is
still used, but only as an **independent reader** to cross-check every
archive ZipShrink writes.

### On-disk files of a job

| File | Content |
|---|---|
| `NAME.zip.partial` | Committed entries, then possibly a half-written entry or central directory (cut off on resume). |
| `NAME.zip.partial.journal` | `Header` (sources, options), `ScanSkip`s, `Plan` entries, `PlanDone`, then `Commit`, `Deleted`, `Kept`, `Failed`, `FinalizeStart`, `Finalized` records. Each line: `<crc32-hex> <json>`. The header is written to `.journal.new`, flushed and renamed, so a journal always starts with a whole header. |
| `NAME.zip` | The finished archive. |

`Commit` records hold the entry's offset, end, method, CRC-32, sizes, SHA-256,
whether its local header has ZIP64 sizes, the CRC-32 of its exact local header
bytes, the metadata written, and the source's identity (size, device, file id,
mtime, change time) at compression time.

### Extra fields written

* `0x0001` ZIP64 (local: both sizes; central: only fields that overflow)
* `0x5455` Info-ZIP extended timestamp (mtime)
* `0x000A` NTFS timestamps (mtime, atime, creation; 100 ns)
* `0x5A53` ZipShrink content hash: `"ZSH1"` + SHA-256 of the uncompressed data
  (files only; ignored by other tools as the spec requires)

Names are UTF-8 with general-purpose bit 11 set. External attributes carry
DOS attributes and, on Unix, the mode in the high 16 bits.

### Concurrency

The writer is single-threaded so the archive stays strictly sequential. With
`threads > 1`, worker threads read and compress **small files (≤ 2 MiB)**
ahead of the writer into memory (bounded to 16 files per thread and 64 MiB),
with the same change detection; the writer appends them in plan order. Large
files are streamed by the writer with a 1 MiB buffer, so memory use does not
depend on file size.

### Split sets

`split::split_scan` assigns files in plan order to N parts, cutting where the
running total reaches k/N of the data while leaving at least one file for
every later part. Folders go with the next file after them; each part also
gets the folder entries above its files. `split::create_jobs` creates all N
journals before anything is compressed, so every part is resumable from the
start; jobs then run one after another.

## Command line (`zipshrink-cli`)

`clap` subcommands `create`, `check`, `resume`, `verify`, `status`, `salvage`,
`update`; `indicatif` progress; Ctrl+C requests a stop at the next safe point
(a second Ctrl+C exits immediately, which is also safe). Exit codes are listed
in `--help`.

## Desktop app (`zipshrink-gui`)

`eframe`/`egui`, wgpu renderer with an OpenGL fallback. All disk and network
work runs on background threads that report through channels; the UI thread
only draws. The late-90s look is drawn by hand in `theme.rs` and `icons.rs`:
3D bevels from 1 px rectangles, anti-aliasing off (`feathering = false`), an
integer UI scale, its own title bar, menu bar and status bar, and the
"ZipShrink Pixel" font — Liberation Sans rasterised with FreeType's
monochrome hinted renderer at 11 px and converted pixel-for-pixel into a
TrueType font (`scripts/make_pixel_font.py`), so egui reproduces the bitmap
exactly at 13 px. `y2k.rs` adds the 1999–2001 "futuristic" layer: a
7-segment LCD readout with unlit ghost segments, a segmented throughput meter,
LED activity lights, and a chrome-on-perspective-grid banner. Every reading comes from real engine events and counters
(throughput is sampled from bytes processed; each light flashes when the engine
reports that operation); nothing animates on its own.

## Updater (`zipshrink-update`)

Queries `https://api.github.com/repos/<owner>/<repo>/releases/latest`
(`<owner>/<repo>` fixed at build time by the release workflow), compares
versions, downloads `ZipShrink-x64-setup.exe` and `SHA256SUMS` over HTTPS
(system certificate store), verifies the hash and starts the installer. It
never runs while a job is running and never installs silently.
