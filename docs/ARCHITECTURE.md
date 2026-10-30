# Architecture

ZipShrink is two 32-bit Windows programs written in C99 — `zipshrink.exe`
(command line) and `zipshrink-gui.exe` (windowed) — sharing one engine. They
load on every Windows from 95 to 11 (see [Windows 95 compatibility](#windows-95-compatibility)).

```
src/
  base.c        runtime: heap, strings, printf subset, UTF-8/UTF-16, console, arguments
  hash.c        CRC-32, SHA-256
  plat.c        file system: Unicode paths on NT, ANSI on 9x; verified delete; uncached read
  failpoint.c   fault injection for tests (compiled out of release builds)
  zipfmt.c      ZIP/ZIP64 records: local and central headers, end records, parsers
  journal.c     the recovery journal
  scan.c        validating the selection and walking it into a plan
  space.c       preflight: sampled ratio, worst case, same-volume analysis, verdict
  split.c       dividing a plan into N balanced, or size-limited, independent ZIP files
  verify.c      streaming inflate, read-back check, whole-archive check, salvage
  engine.c      the job: create, run, batch commit, delete, finalize, resume, report
  update.c      update check and verified installer download (WinINet)
  cli.c         zipshrink.exe
  gui.c uw.c y2k.c   zipshrink-gui.exe: windows, A/W layer, millennium readouts
vendor/miniz/   miniz 3.0.2 deflate and inflate (MIT)
res/            resources: menu, consent dialog, icon, version, manifest
tests/          run.py (end-to-end, fault and crash tests), tests.c (in-process)
packaging/      NSIS installer, MSI (wixl), build script
scripts/        Windows 95 import check, icon generator
```

## Windows 95 compatibility

Three rules keep one binary working from Windows 95 to 11:

1. **No C runtime library.** Current compilers' runtimes import functions
   Windows 95 does not have, so the programs link with `-nostdlib` and use
   `base.c` instead (heap via `HeapAlloc`; `memcpy` and friends; a small
   formatter). miniz is compiled against the same few functions.
2. **Only Windows 95 functions in the import table.** A missing import stops a
   program from loading at all. Every wide-character ("W") function and every
   function newer than Windows 95 (`MoveFileExW`, `GetDiskFreeSpaceExA`,
   `NtSetInformationFile`, `InitCommonControlsEx`, WinINet, `ShellExecuteW`,
   …) is looked up with `GetProcAddress` at run time and used only when
   present. `scripts/check_imports.py` checks every import against a reviewed
   list of Windows 95 exports, in CI and in `packaging/build-windows.sh`.
3. **PE header version 4.0** (OS and subsystem), `-march=i386`.

At run time `zs_is_nt()` picks the code path:

| | NT family (NT 4 – 11) | Windows 95 / 98 / Me |
|---|---|---|
| Paths | UTF-16 with `\\?\` prefixes (long paths) | system code page; unrepresentable names skipped |
| Windows | Unicode (`CreateWindowExW` …) | ANSI |
| Read-back | second handle with `FILE_FLAG_NO_BUFFERING` | cached read |
| Delete | one handle `GENERIC_READ \| DELETE \| FILE_WRITE_ATTRIBUTES`, `FILE_SHARE_READ`: verify, then `NtSetInformationFile(FileDispositionInformation)` | deny-write handle: verify; close; re-check; `DeleteFileA` |
| Rename | `MoveFileExW(MOVEFILE_WRITE_THROUGH)` | `MoveFileA` |
| Free space | `GetDiskFreeSpaceExW` | `GetDiskFreeSpaceExA` (95 OSR2+) or `GetDiskFreeSpaceA` |

All strings inside the program are UTF-8; `plat.c`, `uw.c` and `base.c`
convert at the edges with ZipShrink's own codecs (`CP_UTF8` does not exist on
Windows 95).

## Engine

| Module | Responsibility |
|---|---|
| `scan` | Makes the selection absolute and checks it (no links, no overlapping roots, no drive roots, archive not inside a source); walks it without following links, files before subfolders, into the ordered plan; skips links, offline files, unrepresentable names and over-long paths with a reason. |
| `space` | Preflight: compresses samples of up to 24 files (the 12 largest and 12 spread through the plan, first 512 KiB of each) for a ratio; worst case; which data is on the destination volume (only that is freed by deletion); peak need; free space and reserve; verdict and explanation. |
| `split` | Assigns files in plan order to N parts, cutting where the running total reaches k/N of the data while leaving at least one file for every later part; folders go with the next file; each part gets the folder entries above its files. By size: starts a new part when the next file would take the part's estimated size (sampled ratio plus header overhead) over the limit. |
| `zipfmt` | Hand-written ZIP and ZIP64 records and their parsers. |
| `journal` | Append-only, checksummed, flushed text records with torn-tail detection and an exclusive lock. |
| `engine` | The `Job`: create (journal header and plan), run, batch commit, deletion, finalization, resume, report. |
| `verify` | Streaming inflate with CRC-32 and SHA-256; read-back check of one entry; whole-archive check; salvage. |
| `failpoint` | Named fault-injection points (`-DZS_FAILPOINTS` only). |

### Why a custom ZIP writer

Crash recovery needs to know exactly where each entry begins and ends, and
needs entries that can be found again without a central directory. Every
entry is written as a local header followed by its data — **no data
descriptors** — and the header is rewritten in place once the CRC-32 and
sizes are known. Its length never changes: the ZIP64 extra field is reserved
up front for files that could reach 4 GiB, and the SHA-256 extra field is
written with zeros first. The partial archive is therefore always a sequence
of self-describing entries, which is what both `resume` and `salvage` rely on.

### On-disk files of a job

| File | Content |
|---|---|
| `NAME.zip.partial` | Committed entries, then possibly a half-written entry or central directory (cut off on resume). |
| `NAME.zip.partial.journal` | The recovery journal (below). |
| `NAME.zip` | The finished archive. |

Journal lines are `<crc32 of the rest, 8 hex digits> <type>[<TAB>key=value]...`
with backslash, tab, CR and LF escaped. Types:

| Type | Meaning |
|---|---|
| `H` | Header: format `zipshrink-journal-c1`, job id, output, sources, options, program version and Windows version. Written to `.journal.new`, flushed and renamed, so a journal always starts with a whole header. |
| `s` | A scanned item that will not be archived, and why. |
| `P` | Plan entry: index, root, size, archive name, source path, folder or file, times, attributes. |
| `D` | The plan is complete. |
| `C` | Commit: offset, end, method, CRC-32, sizes, SHA-256, ZIP64-ness and the CRC-32 of the exact local header bytes, metadata, and the source's identity (size, mtime, volume, file id, attributes) when it was read. |
| `X` / `K` / `F` | Original deleted (or already missing) / kept, with the reason / file failed, with the reason. |
| `S` | Finalization started (central directory offset). |
| `Z` | Renamed to the final name. |

A crash can only tear the last line; it is recognised by its checksum or a
missing newline and ignored. Damage anywhere else stops the job.

### Extra fields written

* `0x0001` ZIP64 (local: both sizes; central: only fields that overflow)
* `0x5455` Info-ZIP extended timestamp (mtime)
* `0x000A` NTFS timestamps (mtime, atime, creation; 100 ns)
* `0x5A53` ZipShrink content hash: `"ZSH1"` + SHA-256 of the uncompressed data
  (files only; ignored by other tools as the specification requires)

Names are UTF-8 with general-purpose bit 11 set; external attributes carry
DOS attributes ("made by" MS-DOS).

### Memory and threads

Files are streamed with a 1 MiB buffer, so memory use does not depend on file
size. The engine runs on one thread; the GUI runs it on a worker thread and
receives progress through posted window messages, so the window stays
responsive. Space checks and update checks also run on worker threads.

## Command line (`cli.c`)

Commands `create`, `check`, `resume`, `verify`, `status`, `salvage`,
`update`. Progress is one line redrawn on the console (its bar is plain ASCII,
so it reads right in any code page); text goes to the console as Unicode on
NT and in the OEM code page on 9x, and output redirected to a file is UTF-8. Ctrl+C asks the
job to stop at the next safe point; a second Ctrl+C exits at once, which is
also safe.

## Windowed program (`gui.c`, `uw.c`, `y2k.c`)

Plain Win32 with the classic common controls of Windows 95 (list views,
progress bars, up-down, status bar) in 8-point MS Sans Serif — the real
bitmap font, not an imitation — and no visual-styles manifest, so it keeps
the 1998 look on every version. `uw.c` gives the rest of the code UTF-8 calls
that become Unicode windows on NT and ANSI windows on 9x. `y2k.c` draws the
1999–2001 layer with GDI: a chrome wordmark over a perspective grid, a
seven-segment LCD with unlit ghost segments, a segmented throughput meter and
LED activity lights. Every reading comes from the running job (the meter
samples the engine's transfer rate; each light flashes when the engine
reports that operation); nothing animates on its own.

## Updater (`update.c`)

Loads WinINet at run time, asks
`https://api.github.com/repos/knivesbatz/zipshrink/releases/latest` for the
newest release, compares versions, downloads `SHA256SUMS` and
`ZipShrink-setup.exe`, checks the SHA-256 and starts the installer. A copy
run from a folder holding `zipshrink-portable.txt` opens the release page
instead. It never installs silently or while a job runs.
