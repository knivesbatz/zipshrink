# Architecture

ZipShrink is a Rust workspace that ships as two 64-bit Windows programs,
`zipshrink-gui.exe` (the desktop app) and `zipshrink.exe` (the command line),
in one installer. Both use the same engine.

```
crates/
  zipshrink-core     engine: planning, ZIP writer, journal, verification, recovery
  zipshrink-cli      `zipshrink` command line tool
  zipshrink-gui      `zipshrink-gui` desktop app (egui/eframe)
  zipshrink-update   update check, verified installer download, settings
  zipshrink-setup    `ZipShrink-setup.exe`, the installer and uninstaller
packaging/           the script that builds the installer
scripts/             icon and license generators
```

## Core engine (`zipshrink-core`)

| Module | Responsibility |
|---|---|
| `scan` | Validates and canonicalises the selection (no links, no overlapping roots, no drive roots, archive not inside a source), walks it without following links, and produces the ordered plan. |
| `space` | Preflight: sampled compression ratio, worst case, same-volume analysis, peak need, verdict. |
| `split` | Divides a plan into N balanced parts, or into parts of at most a given size; names them; finds unfinished parts of a set. |
| `format` | Hand-written ZIP/ZIP64 records: local headers (patched in place), central directory, end records, extra fields; parsers for local headers and central directories. |
| `journal` | Append-only checksummed JSON-lines recovery journal with torn-tail detection and an exclusive lock. |
| `engine` | The `Job`: create, run, batch commit, deletion, finalization, resume, report. |
| `verify` | Read-back check of one entry (header bytes, parse, inflate, CRC-32, SHA-256) and of a whole archive with the independent `zip` crate. |
| `salvage` | Rebuilds a readable archive from a partial archive without its journal. |
| `platform` | OS specifics: no-follow opens, Windows share modes, file identity, flush, uncached reads, verified deletion (including read-only files), no-replace rename, volume ids, free space. The Unix implementation exists for development and tests. |
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
DOS attributes.

### Concurrency

The writer is single-threaded so the archive stays strictly sequential. With
`threads > 1`, worker threads read and compress **small files (≤ 2 MiB)**
ahead of the writer into memory (bounded to 16 files per thread and 64 MiB),
with the same change detection; the writer appends them in plan order. Large
files are streamed by the writer with a 1 MiB buffer, so memory use does not
depend on file size.

### Split sets

Files are assigned to parts in plan order and never cut in two:

* `split::split_scan` (N parts) cuts where the running total reaches k/N of
  the data while leaving at least one file for every later part.
* `split::split_scan_size` (parts of a size, at least 64 KiB) starts a new
  part when the next file would take the current part's estimate (sampled
  ratio times size, plus header overhead, plus the folder entries the new
  part repeats) over the limit. A larger file gets a part of its own. It
  returns no parts when everything fits in one archive.

Both then share one step: folders go with the next file after them, and each
part also gets the folder entries above its files. `split::create_jobs`
creates all journals before anything is compressed, so every part is
resumable from the start; jobs then run one after another.

## Command line (`zipshrink-cli`)

`clap` subcommands `create`, `check`, `resume`, `verify`, `status`, `salvage`,
`update`; `indicatif` progress; Ctrl+C requests a stop at the next safe point
(a second Ctrl+C exits immediately, which is also safe). Exit codes are listed
in `--help`.

## Desktop app (`zipshrink-gui`)

`eframe`/`egui`, with an interface modelled on macOS Big Sur. All disk and
network work runs on background threads that report through channels; the
UI thread only draws.

- `src/design/` is the design system, and screens use nothing else:
  - `tokens.rs`: colours (separate light and dark palettes), type scale,
    spacing, radii, shadows and motion timings.
  - `theme.rs`: picks the appearance and cross-fades the palette when it
    changes.
  - `motion.rs`: every animation goes through it, so reduced motion
    (Windows' "Animation effects" setting or the app's own) makes movement
    instant and fades short.
  - `fonts.rs`: SF Pro and SF Mono when the user has them installed,
    otherwise the bundled Inter and Inter Display. The system's fonts fill in
    other scripts.
  - `icons.rs`: SF-Symbols-style vector icons.
  - `chrome.rs`: the frameless window's traffic lights, drag region, resize
    edges and Windows 11 rounded corners.
  - `widgets.rs`: push and pop-up buttons, checkboxes, segmented controls,
    steppers, fields, forms, tables, progress indicators.
- `src/app/` holds the window layout (sidebar, unified toolbar, content) and
  the pages:
  - `compress.rs`: the setup form and estimate.
  - `run.rs`: progress and the result.
  - `verify.rs`, `resume.rs` and `prefs.rs`: the other pages.
  - `sheets.rs`: alert sheets.
  - `toast.rs`: notification banners.
  - `updates.rs`: the update flow.
- `src/instance.rs` keeps one window: Explorer's right-click menu starts one
  process per selected item, and later ones hand their paths to the first
  through `%APPDATA%\ZipShrink\inbox` and exit.

## Updater (`zipshrink-update`)

Queries `https://api.github.com/repos/<owner>/<repo>/releases/latest`
(`<owner>/<repo>` fixed at build time by `ZIPSHRINK_UPDATE_REPO`) over HTTPS
with the system certificate store, and compares versions. Automatic checks
run at most once a week (`CHECK_INTERVAL`). When there is a newer release it downloads
`ZipShrink-setup.exe` to the temporary folder in the background and checks its
SHA-256 against the `digest` GitHub records for that asset, falling back to
the release's `SHA256SUMS` only when there is none; a mismatch deletes the
file. `launch_installer` starts it as `ZipShrink-setup.exe /S`, with
`/RESTART` when the app should open again; the app asks first, and installs
right away (Restart) or when it closes, never while a job runs. Settings (update checks, appearance,
reduced motion) are kept in `%APPDATA%\ZipShrink\settings.json`.

CI builds a `zipshrink.exe` with `ZIPSHRINK_UPDATE_FROM=0.0.1`, so
`zipshrink update` downloads, verifies and installs the published release on
a real Windows runner.

## Installer (`zipshrink-setup`)

`ZipShrink-setup.exe`, built on Linux by `packaging/build-windows.sh`, is a
Rust program that carries the files to install (compressed, embedded at
build time from the folder named by `ZIPSHRINK_SETUP_PAYLOAD`). Its window
follows macOS Big Sur's Installer, built from the app's design module
(`crates/zipshrink-gui/src/design`, included by path): Introduction,
License, Destination Select, Installation Type (Customize lists the
optional parts), Installation and Summary. Copied into the install folder as
`Uninstall.exe`, the same program is the uninstaller.

It replaces the NSIS installer of 0.2-1.0 and keeps its contract, so
upgrades and the 1.0 updater work unchanged: per user into
`%LOCALAPPDATA%\Programs\ZipShrink` (no administrator rights), the folder
recorded in `HKCU\Software\ZipShrink` and the same Add/Remove Programs
values; `/S` silent, `/RESTART` to start the app afterwards, `/D=` for the
folder and `_?=` for the uninstaller; exit codes 0 done, 1 failed, 2
cancelled, 3 Windows older than 10. Optional parts: Start menu and desktop
shortcuts, the Explorer right-click menu, and the `zipshrink` command on
the user's PATH; an upgrade keeps exactly the parts the earlier
installation had. It waits for a running ZipShrink to close (a minute when
silent), writes every new file beside the old one before replacing any,
and removes a copy installed from an earlier `.msi` package.

- `src/ops`: the `Backend` trait; `windows.rs` does the work, `demo.rs`
  pretends (builds without a payload, and non-Windows hosts).
- `src/ui`: the window; `task.rs` runs the install on a thread, and decides
  under one lock whether quitting still cancels it or it has started.
- `src/main.rs`: the command line and the silent mode.
