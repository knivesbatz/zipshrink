# Supported formats and limitations

## Output

* Standard ZIP (PKWARE APPNOTE 6.3), compression **Deflate** (levels 1–9) or
  **Store** (level 0).
* **ZIP64** automatically for files of 4 GiB or more, archives over 4 GiB, and
  more than 65,535 entries.
* UTF-8 names (general-purpose bit 11); folder structure and empty folders;
  modification, access and creation times (DOS time, Info-ZIP extended
  timestamp, NTFS timestamps); DOS attributes; Unix permission bits.
* Per-file SHA-256 in a private extra field, used by `zipshrink verify`.
* Split output is a set of independent ZIP files (`NAME.part1.zip`, …), not a
  spanned/multi-volume archive.

Archives open in Windows Explorer, 7-Zip, WinRAR, macOS Archive Utility,
Info-ZIP `unzip`, Python `zipfile` and others. (Windows Explorer's built-in
ZIP support is slow with very large ZIP64 archives; 7-Zip handles them well.)

## Input

* Any regular files and folders you can read.
* **Skipped and left in place** (reported): symbolic links, Windows junctions,
  mount points and other reparse points (including OneDrive/cloud placeholders
  and deduplicated files), offline files, device files, FIFOs and sockets.
* Names that are not valid Unicode (possible on Linux) cannot be stored as
  UTF-8 and are skipped.
* A single file is never split across parts.

## Not stored in the archive

NTFS alternate data streams, ACLs and security descriptors, Unix ownership,
extended attributes, macOS resource forks and Finder metadata, sparse-file
layout, hard-link relationships (each link is stored as a separate file).
When originals are deleted, this information is lost with them.

## Not supported

* Encryption / passwords.
* Other formats (7z, tar, zstd, Deflate64).
* Updating or appending to an existing ZIP.
* Archives on FAT32 destinations of 4 GiB or more (FAT32 file size limit);
  ZipShrink warns in the preflight and stops cleanly if it happens.

## Platforms

| Platform | Status |
|---|---|
| Windows 10 / 11, x64 | Primary target. Installer, MSI, portable build, CLI. |
| Windows 7 / 8.1 | Not supported (current Rust toolchains require Windows 10). The MSI refuses versions older than 8.1. |
| Windows 95 / 98 / ME / NT / 2000 / XP | Not possible: different kernel or missing APIs, no 64-bit, and no support in Rust's standard library or the graphics stack. The retro look is cosmetic. |
| Linux x86_64 | Supported (CLI and GUI tarball). |
| macOS (Apple Silicon and Intel) | Supported (CLI and GUI tarball; unsigned). |

## Performance notes

* Compression of large files uses one core; small files are compressed in
  parallel (`-j`).
* With deletion on, each file is read twice by default (once to compress,
  once to confirm it did not change before deletion) and each entry is read
  back once from the archive; `--quick-change-check` skips the second read of
  the original. The finished archive is decompressed once more by default
  (`--no-final-verify` skips it). Integrity is preferred over speed.
* Batches are flushed every 64 MiB / 1,024 files / 3 seconds, so many tiny
  files do not cost one flush each.
