# Data safety

ZipShrink's defining feature deletes your files. This document states exactly
when that happens, what is checked first, what happens on every kind of
failure, and what ZipShrink cannot protect against.

## The rule

> An original is deleted only after its compressed copy is on the storage
> device, has been read back and proven identical, and is recorded in a
> journal that is itself on the storage device — and only if the original is
> still exactly the file that was compressed.

Deletion is **off by default**. The app requires ticking a checkbox and
confirming a warning dialog (with an "I understand" box); the command line
requires `--delete-originals` and then asks you to type `delete` (unless you
pass `--yes`, or standard input is not a terminal).

## Per-batch transaction

Files are processed in plan order and committed in batches (at most 64 MiB
of source data, 1,024 entries, or 3 seconds; a large file is its own batch).

| Step | What happens | If it fails |
|---|---|---|
| 1. Open | No links followed (`O_NOFOLLOW`; `FILE_FLAG_OPEN_REPARSE_POINT` + attribute check). On Windows the file is opened with `FILE_SHARE_READ` only: nobody can write, rename or delete it while it is read. | File reported as failed, left untouched, retried on resume. |
| 2. Compress | Read once; CRC-32 and SHA-256 computed from the bytes read; Deflate (or Store) output appended to `NAME.zip.partial`. Free space checked every 16 MiB written. | Entry cut off the archive; job pauses (disk full / reserve reached) or file is reported as failed. |
| 3. Change check | Size, mtime, change time (Unix ctime / NTFS ChangeTime), device and file id compared before and after reading; bytes read must equal the size. | Re-read up to 3 times, then reported as failed and kept. |
| 4. Header patch | CRC-32, sizes and SHA-256 written into the entry's local header. | As step 2. |
| 5. Flush | `fsync` (`F_FULLFSYNC` on macOS) / `FlushFileBuffers`. | Batch cut off; job stops. Nothing deleted. |
| 6. Read back | Each entry read from the device: Windows uses a second `FILE_FLAG_NO_BUFFERING` handle; Linux evicts the range from the page cache (`POSIX_FADV_DONTNEED`) first; macOS sets `F_NOCACHE`. The header bytes are compared (CRC of the exact header), the header parsed, the data decompressed, and length, CRC-32 and SHA-256 compared with step 2. | Batch cut off; job stops with "verification failed — storage may be failing". Nothing deleted. |
| 7. Journal commit | Commit records (offsets, sizes, checksums, source identity) appended to `NAME.zip.partial.journal` and flushed. | Job stops. Nothing deleted. Resume finds whatever reached the disk. |
| 8. Delete | Only with deletion on. The path must lie inside a selected folder; read-only files are kept unless allowed. The file is opened again; identity, size, times and (unless `--quick-change-check`) a fresh SHA-256 must match the committed values. **Windows:** verified and deleted through one handle opened `GENERIC_READ \| DELETE` with `FILE_SHARE_READ`, via `FileDispositionInfoEx` (POSIX semantics), so it cannot be swapped between check and delete. **Unix:** after hashing, the path must still name the same inode before `unlink`. | Original kept, with the reason shown ("it changed", "read-only", "another program has it open", "could not be deleted: …"). |
| 9. Record | `Deleted` / `Kept` records appended, flushed at the end of the batch. | A crash here means resume sees the original missing and records it as deleted (its verified copy is committed). |

Folders are removed only at the very end, deepest first, only when empty, and
only if they are still the real folders that were scanned.

## Finalization

1. A `FinalizeStart` record is journaled.
2. The central directory (ZIP64 records when needed) is appended and flushed.
3. The whole archive is checked with an **independent ZIP reader** (the `zip`
   crate): entry count, names, sizes and CRC-32 must agree with ZipShrink's own
   central directory parser and with the journal; by default every entry is
   decompressed again and its SHA-256 compared.
4. `NAME.zip.partial` is renamed to `NAME.zip` without replacing anything
   (`renameat2(RENAME_NOREPLACE)`, `renamex_np(RENAME_EXCL)`, `MoveFileExW`
   with `MOVEFILE_WRITE_THROUGH`), unless overwriting was confirmed.
5. `Finalized` is journaled, emptied folders removed, the journal deleted.

## Recovery

`resume` (or **File → Resume Interrupted Job** in the app):

* reads the journal; a torn final record (power cut mid-write) is detected by
  its checksum or missing newline and ignored; damage anywhere else stops
  everything;
* checks the partial archive is at least as long as the committed data and
  that every committed local header is byte-for-byte what was written;
* cuts off everything after the last committed entry (a half-written entry,
  or a half-written central directory);
* for committed entries whose originals were not yet deleted: **re-verifies
  the archived data from the device**, then re-checks the original (always
  with a full SHA-256) before deleting;
* continues compressing, then finalizes.

If the partial archive is shorter than its journal, a committed header has
changed, or an archived copy fails re-verification, ZipShrink stops, deletes
nothing more, and points you to:

* `zipshrink status NAME.zip.partial` — what the journal and archive hold;
* `zipshrink salvage NAME.zip.partial` — walks the local headers, keeps every
  entry whose data decompresses to exactly the recorded size, CRC-32 and
  SHA-256, skips damaged regions (searching for the next valid header), writes
  a new central directory, verifies the result, and renames it.

Partial archives and journals are never deleted automatically after a failure.

## Disk space

* Preflight: total size, sampled compression ratio, worst case (nothing
  compresses), which data is on the destination volume (only that is freed
  by deletion), peak space needed, free space, and a verdict.
* A free-space reserve (default 256 MiB, minimum 16 MiB) is never crossed:
  ZipShrink checks before each file and every 16 MiB written, and pauses
  safely. If a batch is pending it is committed (and its originals deleted)
  first, which may free enough space to continue.
* `ENOSPC`/`EDQUOT`/`ERROR_DISK_FULL` mid-write: the entry is cut off and the
  job pauses; resume when space is available.

## How this is tested

* In-process fault injection (`--features failpoints`): I/O errors and
  disk-full injected at every transaction step and at several hit counts;
  files modified while read (Unix) or locked out (Windows); files changed after
  archiving; unreadable and undeletable files; corrupted partial archive data
  and headers; truncated partial archives; interrupted planning, finalization,
  rename and cleanup; a lost journal recovered with `salvage`.
* Process-level crash tests: the real `zipshrink` executable is killed with
  `abort()` (no cleanup, like a crash or forced termination) at 15 named
  points, including a deliberately torn journal write, then resumed —
  including repeated crashes during resume.
* After every scenario the tests assert that **every original is either still
  present unchanged or in the archive with an identical SHA-256**, and for every
  successful run that extracting the archive with an independent reader
  reproduces every original's SHA-256 exactly. "The ZIP opens" is never enough.
* The same suites run on Linux, macOS and Windows in CI (and were run against
  the Windows build under Wine during development).

## What ZipShrink cannot protect against

* **Lying hardware.** If a drive acknowledges a flush it has not done (some
  consumer drives and USB bridges with volatile write caches), a power cut can
  still lose data that was "verified". The read-back bypasses the OS cache but
  cannot see inside the drive's own cache.
* **Damage after the job.** Once originals are deleted the archive is the only
  copy. Keep backups of irreplaceable data.
* **Hostile local processes on Unix.** Unix has no mandatory locking, so a
  program writing to a file at the instant it is checked could race the
  delete; ZipShrink detects any change made before its final check (ctime
  moves on every write), and the archive always holds the verified version.
* **Network filesystems** may not honour flush, file id or locking semantics.
  Prefer local disks when deleting originals.
* **Metadata not stored in ZIP** (see FORMATS_AND_LIMITATIONS.md) is lost with
  the original: ACLs, alternate data streams, extended attributes, ownership.
