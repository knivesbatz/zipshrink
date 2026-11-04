# Supported formats and limitations

## Output

* Standard ZIP (PKWARE APPNOTE 6.3), compression **Deflate** (levels 1–9) or
  **Store** (level 0).
* **ZIP64** automatically for files of 4 GiB or more, archives over 4 GiB, and
  more than 65,535 entries.
* UTF-8 names (general-purpose bit 11); folder structure and empty folders;
  modification, access and creation times (DOS time, Info-ZIP extended
  timestamp, NTFS timestamps); DOS attributes.
* Per-file SHA-256 in a private extra field, used by `zipshrink verify`.
* Split output is a set of independent ZIP files (`NAME.part1.zip`, …), not a
  spanned/multi-volume archive. Split into a number of parts (2–999) or into
  parts of about a given size (64 KB or more; at most 999 parts).

Archives open in Windows Explorer, 7-Zip, WinZip, WinRAR, Info-ZIP `unzip`,
Python `zipfile` and others. (Windows Explorer's built-in ZIP support is slow
with very large ZIP64 archives; 7-Zip handles them well.) Tools that predate
ZIP64 open archives under 4 GiB with fewer than 65,535 entries; tools that
predate UTF-8 names show non-ASCII names in their own code page.

## Input

* Any regular files and folders you can read.
* **Skipped and left in place** (reported): junctions, symbolic links, mount
  points and other reparse points (including OneDrive/cloud placeholders and
  deduplicated files), offline files, and paths too long for a ZIP name.
* Names that are not valid UTF-16 (unpaired surrogates).
* A single file is never split across parts.

## Not stored in the archive

NTFS alternate data streams, ACLs and security descriptors, ownership,
extended attributes, sparse-file layout, hard-link relationships (each link is
stored as a separate file). When originals are deleted, this information is
lost with them.

## Not supported

* Encryption / passwords.
* Other formats (7z, tar, zstd, Deflate64).
* Updating or appending to an existing ZIP.
* Archives of 4 GiB or more on FAT16/FAT32 destinations (their file size
  limit); ZipShrink warns in the space check, and `--part-size` (for example
  `--part-size 4000M`) keeps each archive smaller.
* Resuming unfinished jobs started by ZipShrink 0.2–0.4 (a different journal
  format); `zipshrink salvage` rebuilds a ZIP from their partial archive.

## Platforms

64-bit Windows 10 and 11 (x64; Arm64 Windows 11 through emulation). The
installer refuses older and 32-bit Windows. The code also builds and is
tested on Linux, but only Windows is shipped.

## Performance notes

* The archive is written by one thread, so it stays strictly sequential and
  recoverable; small files are read and compressed ahead of it in parallel
  (`-j`), large files use one core.
* With deletion on, each file is read twice by default (once to compress,
  once to confirm it did not change before deletion) and each entry is read
  back once from the archive; `--quick-change-check` skips the second read of
  the original. The finished archive is decompressed once more by default
  (`--no-final-verify` skips it). Integrity is preferred over speed.
* Batches are flushed every 64 MiB / 1,024 files / 3 seconds, so many tiny
  files do not cost one flush each.
