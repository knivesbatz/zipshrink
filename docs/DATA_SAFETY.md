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
confirming a warning dialog whose Yes button stays disabled until an "I
understand" box is ticked; the command line requires `--delete-originals` and
then asks you to type `delete` (unless you pass `--yes`, or standard input is
not a terminal, in which case it warns and proceeds because the flag was
given explicitly).

## Per-batch transaction

Files are processed in plan order and committed in batches (at most 64 MiB
of source data, 1,024 entries, or 3 seconds; a large file is its own batch).

| Step | What happens | If it fails |
|---|---|---|
| 1. Open | Opened with `FILE_FLAG_OPEN_REPARSE_POINT` and checked not to be a link, folder or reparse point, with `FILE_SHARE_READ` only: no other program can write to it while it is read. | File reported as failed, left untouched, retried on resume. |
| 2. Compress | Read once; CRC-32 and SHA-256 computed from the bytes read; Deflate (or Store) output appended to `NAME.zip.partial`. Free space checked before each file and every 16 MiB written. | Entry cut off the archive; the job pauses (disk full / reserve reached) or the file is reported as failed. |
| 3. Change check | Size, modification time, change time (NTFS ChangeTime), volume serial number and file id compared before and after reading; the bytes read must equal the size. | Re-read up to 3 times, then reported as failed and kept. |
| 4. Header patch | CRC-32, sizes and SHA-256 written into the entry's local header (same length as the placeholder). | As step 2. |
| 5. Flush | `FlushFileBuffers` on the archive. | Batch cut off; job stops. Nothing deleted. |
| 6. Read back | Each entry read again through a second handle opened with `FILE_FLAG_NO_BUFFERING`, so the data comes from the device, not the cache. The header bytes are compared (CRC-32 of the exact header), the header parsed, the data decompressed, and length, CRC-32 and SHA-256 compared with step 2. | Batch cut off; job stops with "verification failed". Nothing deleted. |
| 7. Journal commit | Commit records (offsets, sizes, checksums, source identity) appended to `NAME.zip.partial.journal` and flushed. | Job stops. Nothing deleted. Resume finds whatever reached the disk. |
| 8. Delete | Only with deletion on. The path must lie inside a selected folder; read-only files are kept unless allowed. The file is opened again; size, times and identity, and (unless `--quick-change-check`) a fresh SHA-256 of its contents, must match the committed values. This happens through one handle opened `GENERIC_READ \| DELETE` with `FILE_SHARE_READ`, and the same handle deletes the file (`FileDispositionInfoEx` with POSIX semantics, or `FileDispositionInfo` where that is missing), so it cannot be changed or swapped between check and delete. | Original kept, with the reason shown ("it changed", "read-only", "another program has it open", "could not be deleted: …"). |
| 9. Record | `X` (deleted) / `K` (kept) records appended and flushed at the end of the batch. | A crash here means resume finds the original missing and records it as already gone (its verified copy is committed). |

A read-only file, when deleting read-only files is allowed, is deleted
through the same handle with `FILE_DISPOSITION_FLAG_IGNORE_READONLY_ATTRIBUTE`
(or, on file systems without it, with its read-only attribute cleared
through that handle and restored if the delete fails). Where opening a
read-only file for deletion is refused outright (some network shares, Wine),
ZipShrink checks it is still the archived file, clears the attribute by
name, tries once more with the full check, and puts the attribute back if
the file is kept.

Folders are removed only at the very end, deepest first, only when empty, and
only if they lie inside a selected folder.

## Finalization

1. An `S` record is journaled.
2. The central directory (with ZIP64 records when needed) is appended and
   flushed.
3. The whole archive is read back: the central directory is parsed, every
   local header is checked against it, and by default every entry is
   decompressed again with its CRC-32 and SHA-256 compared. Every
   central-directory record must also match the journal's commit for that
   entry (name, offset, sizes, CRC-32, SHA-256).
4. `NAME.zip.partial` is renamed to `NAME.zip` without replacing anything
   (`MoveFileExW` with `MOVEFILE_WRITE_THROUGH`), unless
   overwriting was confirmed.
5. `Z` is journaled, emptied folders removed, the journal deleted.

The finished archive is also read with an independent ZIP reader (the `zip`
crate). The test suite checks every archive that way and compares the
SHA-256 of every extracted file.

## Recovery

`resume` (or resuming the job in the app):

* reads the journal; a torn final record (power cut mid-write) is detected by
  its checksum or missing newline and ignored; damage anywhere else stops
  everything;
* takes an exclusive lock on the journal, so two copies of ZipShrink cannot
  work on the same job;
* checks the partial archive is at least as long as the committed data and
  that every committed local header is byte-for-byte what was written;
* cuts off everything after the last committed entry (a half-written entry,
  or a half-written central directory);
* for committed entries whose originals were not yet deleted: **re-verifies
  the archived data**, then re-checks the original (always with a full
  SHA-256) before deleting;
* continues compressing, then finalizes.

If the partial archive is shorter than its journal, a committed header has
changed, or an archived copy fails re-verification, ZipShrink stops, deletes
nothing more, and points you to:

* `zipshrink status NAME.zip.partial` — what the journal and archive hold;
* `zipshrink salvage NAME.zip.partial` (or salvaging it in the app) — walks the
  local headers, keeps every entry whose data decompresses to exactly the
  recorded size, CRC-32 and SHA-256, skips damaged regions (searching for the
  next valid header), writes a new central directory, verifies the result,
  and renames it to the archive name. It works in place, so it needs no extra
  disk space; the only bytes it removes are an unfinished entry at the end,
  which holds no complete file.

Partial archives and journals are never deleted automatically after a failure.

Jobs left unfinished by ZipShrink 0.2–0.4 used a different journal format.
Version 1.0 recognises it, deletes nothing, and says to run `salvage`, which
rebuilds a ZIP of every file the partial archive holds; files that are not in
it are still in their original place.

## Disk space

* Preflight: total size, sampled compression ratio, worst case (nothing
  compresses), which data is on the destination volume (only that is freed
  by deletion), peak space needed, free space, and a verdict.
* A free-space reserve (default 256 MiB, minimum 16 MiB) is never crossed:
  ZipShrink checks before each file and every 16 MiB written, and pauses
  safely. If a batch is pending it is committed (and its originals deleted)
  first, which may free enough space to continue.
* "Disk full" in the middle of a write: the entry is cut off and the job
  pauses; resume when space is available.

## How this is tested

* In-process fault injection (`--features failpoints`): I/O errors and
  disk-full injected at every transaction step and at several hit counts;
  files changed while read or after archiving; unreadable and undeletable
  files; read-only files; corrupted partial archive data and headers;
  truncated partial archives; interrupted planning, finalization, rename and
  cleanup; a lost journal recovered with `salvage`.
* Process-level crash tests: the real `zipshrink` executable is killed with
  `abort()` (no cleanup, like a crash or forced termination) at every named
  point of the transaction, including a deliberately torn journal write, then
  resumed — including repeated crashes during resume.
* After every scenario the tests assert that **every original is either still
  present unchanged or in the archive with an identical SHA-256**, and for every
  successful run that extracting the archive with an independent reader
  reproduces every original's SHA-256 exactly. "The ZIP opens" is never enough.
* CI runs the suites on Windows and Linux, and a round trip with deletion
  and splitting checked by 7-Zip.

## What ZipShrink cannot protect against

* **Lying hardware.** If a drive acknowledges a flush it has not done (some
  consumer drives and USB bridges with volatile write caches), a power cut can
  still lose data that was "verified". The read-back bypasses the Windows
  cache but cannot see inside the drive's own cache.
* **Damage after the job.** Once originals are deleted the archive is the only
  copy. Keep backups of irreplaceable data.
* **Network drives** may not honour flush, file id or locking semantics.
  Prefer local disks when deleting originals.
* **Metadata not stored in ZIP** (see FORMATS_AND_LIMITATIONS.md) is lost with
  the original: ACLs, alternate data streams, extended attributes, ownership.
