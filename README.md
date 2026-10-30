# ZipShrink

**Compress a folder into a ZIP without needing room for both.**

ZipShrink creates standard ZIP archives and, if you ask it to, deletes each
original file as soon as its compressed copy has been written to disk, read
back, and proven identical. The space the originals used is reclaimed while
the archive grows, so zipping a 200 GB folder needs a little over the space
of one file, not another 200 GB.

It is a free, open-source desktop app and command-line tool for Windows 10
and 11, with the same engine building on macOS and Linux. It looks like it
shipped in 1998 — on purpose — but underneath it is a modern, crash-safe,
fully verified archiver.

![ZipShrink main window](docs/screenshots/main-window.png)

> ZipShrink is an independent project. It does the reverse of what
> [HatchZip](https://hatchzip.dev) does (HatchZip extracts archives in place)
> and is not affiliated with HatchZip or its author.

## Contents

- [Why](#why)
- [How it works](#how-it-works)
- [Is it safe?](#is-it-safe)
- [Install](#install)
- [Using the app](#using-the-app)
- [Using the command line](#using-the-command-line)
- [Splitting into several ZIPs](#splitting-into-several-zips)
- [If something goes wrong](#if-something-goes-wrong)
- [Updates](#updates)
- [Limitations](#limitations)
- [Building from source](#building-from-source)
- [Documentation](#documentation)
- [License](#license)

## Why

A normal ZIP tool reads your files and writes the archive next to them. Until
it finishes, the disk holds the originals **and** the archive. On a nearly full
disk that is impossible, and the usual workaround — zip to another drive, then
copy back — needs a second drive.

ZipShrink processes files in small batches and frees each batch's originals as
soon as they are safely in the archive:

| Folder size | Typical ZIP tool needs | ZipShrink with deletion needs |
|---|---|---|
| 200 GB of game files that compress to 150 GB | 150 GB free | about the size of the largest file, plus a small reserve |

## How it works

For every batch of files (at most ~64 MB of data, 1,024 files or 3 seconds;
a large file is a batch of its own), ZipShrink:

1. opens each file without following links (on Windows it also stops other
   programs from writing to it while it is read);
2. reads it once, computing its **CRC-32** and **SHA-256** while compressing
   it (Deflate) onto the end of `NAME.zip.partial`;
3. checks the file did not change while it was read (size, modification time,
   change time, file id); if it did, it re-reads it;
4. **flushes** the archive to the storage device (`fsync` / `FlushFileBuffers`);
5. **reads every entry back from the device** — bypassing the file cache where
   the OS allows — parses its header, decompresses it and compares the result
   with the CRC-32 and SHA-256 taken from the original;
6. records the batch in a **durable recovery journal** (`NAME.zip.partial.journal`,
   flushed to disk);
7. only then, if deletion is on, checks each original once more — same file
   id, size and times, and by default the same SHA-256 by re-reading it — and
   **permanently deletes it**. On Windows the check and the delete happen
   through one locked handle, so nothing can swap the file in between.

When every file is done, ZipShrink writes the ZIP central directory, verifies
the whole archive again with an independent ZIP reader (every entry
decompressed, CRC-32 and SHA-256 checked), renames `NAME.zip.partial` to
`NAME.zip`, and removes the folders that were emptied.

Archives are plain ZIP (ZIP64 when files or archives exceed 4 GB or 65,535
entries) and open in Windows Explorer, 7-Zip, WinRAR, macOS Archive Utility,
`unzip` and every other ZIP tool. ZipShrink stores each file's SHA-256 in an
extra field that other tools ignore, so `zipshrink verify` can check archives
it made far more strictly than CRC-32 alone.

## Is it safe?

Deleting originals is the point of the tool, so it is built around never
deleting anything it cannot prove is safely archived.

- **Deletion is off by default.** In the app you must tick the option and
  confirm a warning; on the command line you must pass `--delete-originals`
  (and confirm at the prompt unless you add `--yes`).
- **Nothing is deleted on the strength of a memory buffer.** Data must be on
  the disk, read back, decompressed and matched by two checksums, and the
  journal must be on the disk, before the original is touched.
- **Changed files are kept.** A file modified after it was compressed is not
  deleted; the archive holds the version that was verified.
- **Links are never followed.** Symbolic links, junctions and other reparse
  points (including OneDrive placeholders) are skipped and left in place.
  Nothing outside the folders you selected is read or deleted.
- **Read-only files are kept** unless you allow them to be deleted.
- **Crashes, power cuts and full disks are survivable.** Everything committed
  stays in the partial archive; `resume` cuts off any half-written entry and
  carries on. Temporary archives and journals are never deleted after a
  failure.
- **A damaged partial archive stops all deletion.** Before deleting anything
  on resume, ZipShrink re-verifies the archived copy. If anything does not
  match, it stops and tells you how to recover.
- **The disk is never filled.** You choose a free-space reserve (256 MB by
  default, never below 16 MB); ZipShrink pauses safely before crossing it.
- **Existing archives are never overwritten** without confirmation, and even
  then only once the new archive is complete and verified.

These guarantees are tested: the test suite kills the real program at every
critical step (before and after the journal commit, before and after a
deletion, during finalization, with torn journal writes, with injected disk
full errors), resumes it, and checks that every original is either still
present or in the final archive with an identical SHA-256.

**Still: deleted originals are gone.** After the job, the archive is the only
copy. Keep backups of anything irreplaceable, and do not use deletion on
storage you suspect is failing. See [docs/DATA_SAFETY.md](docs/DATA_SAFETY.md)
for the full design and its limits.

## Install

**Windows 10 and 11 (64-bit)** — from the [latest release](https://github.com/knivesbatz/zipshrink/releases/latest):

| File | What it is |
|---|---|
| `ZipShrink-x64-setup.exe` | Installer for your user account; no administrator rights needed. Adds a Start menu entry, an optional "Compress with ZipShrink…" right-click menu, and the `zipshrink` command. |
| `ZipShrink-x64.msi` | The same app as a per-machine Windows Installer package, for IT deployment: `msiexec /i ZipShrink-x64.msi /qn` |
| `ZipShrink-x64-portable.zip` | Nothing to install: unzip anywhere and run `zipshrink-gui.exe`. |
| `zipshrink-x86_64-windows.exe` | Just the command-line tool. |

The programs are not code-signed yet, so Windows SmartScreen may say "Windows
protected your PC": choose **More info**, then **Run anyway**. Every release
lists SHA-256 checksums in `SHA256SUMS`.

**Older Windows versions are not supported.** Windows 95/98/ME use a
different kernel without the Unicode, 64-bit and NTFS features ZipShrink
depends on, and current Rust toolchains require Windows 10 or later. (The
1990s look is only skin-deep.)

**macOS and Linux** — download `zipshrink-<version>-<target>.tar.gz` from the
release (it contains `zipshrink` and `zipshrink-gui`), or build from source.

## Using the app

1. Drag files and folders onto the window, or use **Add Folder…** / **Add Files…**.
2. Choose where to save the archive, its name, the compression level, and
   optionally **Split into** more than one ZIP.
3. Tick **Delete original files after successful compression** if you want
   the space back as you go, and confirm.
4. Check the **Disk space** box: it shows the estimated archive size (from
   compressing samples of your files), how much space the job needs, how much
   is free, and whether it fits.
5. Click **Compress** (or **Compress & Delete**).

![Deletion must be confirmed](docs/screenshots/delete-confirmation.png)

While it runs you see the current file and its status (Compressing,
Verifying, Safely archived, Original deleted, Skipped, Failed), overall
progress, archive size, estimated remaining size, free space, space
reclaimed, files done and remaining, compression ratio and speed. **Pause**
stops at the next safe point; you can resume later, even after closing the app.

![Progress](docs/screenshots/progress.png)

When it finishes you get the archive location, original and final sizes, space
reclaimed, files archived, originals deleted, anything skipped or kept and why,
and the verification result, with **Open Folder** and **Verify Again**.

![Finished](docs/screenshots/finished.png)

## Using the command line

```
zipshrink create ./GameFiles --output ./GameFiles.zip
zipshrink create ./GameFiles --output ./GameFiles.zip --delete-originals
zipshrink check  ./GameFiles --output ./GameFiles.zip          # changes nothing
zipshrink resume ./GameFiles.zip.partial
zipshrink verify ./GameFiles.zip
```

Useful options for `create` (see `zipshrink create --help`):

| Option | Meaning |
|---|---|
| `-l, --level 0-9` | 0 stores, 1 fastest, 6 default, 9 smallest |
| `-p, --parts N` | split into N separate ZIP files |
| `--delete-originals` | delete each original after it is verified (asks to confirm; `--yes` skips the question) |
| `--min-free SIZE` | free space to keep on the destination, e.g. `2G` (default `256M`) |
| `--quick-change-check` | before deleting, compare size, times and file id instead of re-reading the whole file |
| `--delete-readonly` | also delete read-only files |
| `--overwrite` | replace an existing archive (only after the new one is verified) |
| `-j, --threads N` | worker threads for compressing small files |
| `--no-final-verify` | skip the second full read of the finished archive |

Other commands: `status` shows what an unfinished job holds; `salvage`
rebuilds a readable ZIP from a `.partial` file whose journal is lost;
`update` checks for and installs a newer version.

Exit codes: `0` success · `1` error · `2` bad command line · `3` finished but
some files were skipped, failed or kept · `4` not enough disk space · `5`
interrupted (resumable) · `6` verification failed · `7` partial archive or
journal damaged.

## Splitting into several ZIPs

Set **Split into** in the app, or `--parts N` on the command line, to produce
`NAME.part1.zip`, `NAME.part2.zip`, … Before starting, ZipShrink shows the
estimated and maximum size of every part.

![Split preview](docs/screenshots/split-and-delete.png)

Each part is a **complete, independent ZIP** — not a spanned `.z01` set — so
each opens on its own, and extracting all of them into one folder rebuilds the
original tree. Files are divided in order at file boundaries so the parts are
as even as possible; a single file is never cut in two, so one very large file
makes its part larger. Every part is its own crash-safe job, and `resume`
continues all unfinished parts of a set.

## If something goes wrong

- **The app or computer crashed, or you paused:** run the app and choose
  **File → Resume Interrupted Job…** (or open the `.partial` file with it), or
  run `zipshrink resume NAME.zip.partial`.
- **The disk filled up / reached the reserve:** free some space (or lower the
  reserve with `--min-free`) and resume.
- **ZipShrink says the partial archive or journal is damaged:** it has stopped
  deleting. Keep both files. `zipshrink status NAME.zip.partial` shows what
  they hold; `zipshrink salvage NAME.zip.partial` rebuilds a readable ZIP from
  every intact entry (each checked by CRC-32 and SHA-256).

## Updates

The app checks GitHub for a newer release at most once a day (turn this off
under **Help → Check for Updates Automatically**). It never installs anything
by itself: when you click **Update Now**, it downloads the installer, checks
its SHA-256 against the `SHA256SUMS` file published with the release, and runs
it. Portable copies and macOS/Linux open the release page instead. From the
command line: `zipshrink update` (or `zipshrink update --check`). Updates are
never offered while a job is running.

## Limitations

See [docs/FORMATS_AND_LIMITATIONS.md](docs/FORMATS_AND_LIMITATIONS.md). In short:
output is ZIP with Deflate or Store (no encryption, no 7z/zstd); names must be
valid Unicode; NTFS alternate data streams, ACLs and extended attributes are
not stored; links are skipped; FAT32 destinations cannot hold archives of 4 GB
or more; the Windows build is tested on Windows 10/11 x64.

## Building from source

Requires Rust 1.89 or later.

```
cargo build --release
cargo test --workspace
cargo test -p zipshrink-core -p zipshrink-cli --features zipshrink-core/failpoints,zipshrink-cli/failpoints   # crash tests
```

Windows release files can be built on Linux with `packaging/build-windows.sh`
(MinGW, NSIS and wixl). See [docs/BUILDING.md](docs/BUILDING.md).

## Documentation

- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) — modules, on-disk formats, transaction design
- [docs/DATA_SAFETY.md](docs/DATA_SAFETY.md) — guarantees, failure handling, what is and is not protected
- [docs/FORMATS_AND_LIMITATIONS.md](docs/FORMATS_AND_LIMITATIONS.md)
- [docs/BUILDING.md](docs/BUILDING.md)
- [THIRD_PARTY_LICENSES.md](THIRD_PARTY_LICENSES.md)

## License

MIT — see [LICENSE](LICENSE). Third-party components are listed with their
licenses in [THIRD_PARTY_LICENSES.md](THIRD_PARTY_LICENSES.md); all are
permissive. The pixel font is derived from Liberation Sans (SIL OFL 1.1).
