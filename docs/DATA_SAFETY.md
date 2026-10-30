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
| 3. Change check | Size and modification time — and on the NT family the volume serial number and file id — compared before and after reading; the bytes read must equal the size. | Re-read up to 3 times, then reported as failed and kept. |
| 4. Header patch | CRC-32, sizes and SHA-256 written into the entry's local header (same length as the placeholder). | As step 2. |
| 5. Flush | `FlushFileBuffers` on the archive. | Batch cut off; job stops. Nothing deleted. |
| 6. Read back | Each entry read again: on the NT family through a second handle opened with `FILE_FLAG_NO_BUFFERING`, so the data comes from the device, not the cache. The header bytes are compared (CRC-32 of the exact header), the header parsed, the data decompressed, and length, CRC-32 and SHA-256 compared with step 2. | Batch cut off; job stops with "verification failed". Nothing deleted. |
| 7. Journal commit | Commit records (offsets, sizes, checksums, source identity) appended to `NAME.zip.partial.journal` and flushed. | Job stops. Nothing deleted. Resume finds whatever reached the disk. |
| 8. Delete | Only with deletion on. The path must lie inside a selected folder; read-only files are kept unless allowed. The file is opened again; size, times and identity, and (unless `--quick-change-check`) a fresh SHA-256 of its contents, must match the committed values. On the NT family this happens through one handle opened `GENERIC_READ \| DELETE` with `FILE_SHARE_READ`, and the same handle deletes the file (`FileDispositionInformation`), so it cannot be changed or swapped between check and delete. | Original kept, with the reason shown ("it changed", "read-only", "another program has it open", "could not be deleted: …"). |
| 9. Record | `X` (deleted) / `K` (kept) records appended and flushed at the end of the batch. | A crash here means resume finds the original missing and records it as already gone (its verified copy is committed). |

A file that is only allowed to be deleted with **Also delete read-only
files** has its read-only attribute cleared through the same handle just
before the delete, and restored if the delete fails.

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
   (`MoveFileExW` with `MOVEFILE_WRITE_THROUGH` on the NT family), unless
   overwriting was confirmed.
5. `Z` is journaled, emptied folders removed, the journal deleted.

The test suite checks every archive with an independent reader (Python's
`zipfile`) and the SHA-256 of every extracted file.

## Recovery

`resume` (or **File → Resume Unfinished Job…** in the app):

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
* `zipshrink salvage NAME.zip.partial` (or **File → Salvage**) — walks the
  local headers, keeps every entry whose data decompresses to exactly the
  recorded size, CRC-32 and SHA-256, skips damaged regions (searching for the
  next valid header), writes a new central directory, verifies the result,
  and renames it to the archive name. It works in place, so it needs no extra
  disk space; the only bytes it removes are an unfinished entry at the end,
  which holds no complete file.

Partial archives and journals are never deleted automatically after a failure.

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

## Windows 95, 98 and Me

These systems have no Unicode file API, no uncached reads through the file
system, and no way to delete a file through an open handle. On them:

* names outside the system code page are skipped and left in place (never
  guessed at, so the wrong file can never be opened or deleted);
* the read-back in step 6 is served from the file cache after the flush, so
  it proves the data was written correctly through the file system and
  compresses back to the original, but not that the device returns it;
* in step 8 the original is opened with a handle that keeps writers out,
  hashed and compared, then the handle is closed, the size and modification
  time are checked once more, and the file is deleted by name. A program
  that replaced the file with one of identical size and time in that instant
  could have the replacement deleted; the archive still holds the verified
  original.

Everything else — batching, flushing, the journal, resume, salvage, the free
space reserve — works the same.

## How this is tested

* `tests/tests.c` (in-process): CRC-32 and SHA-256 test vectors, the journal
  (escaping, torn tail, damage), streaming inflate of a corrupted stream,
  split balance, and the engine keeping an original whose contents changed
  after it was archived even though its size and time were put back.
* `tests/run.py` (whole programs): round trips at several levels, ZIP64,
  1,500-file jobs, several sources, split sets, read-only files, unreadable
  files, existing outputs, outputs inside a source, the space check, and
  corrupted and truncated archives for `verify`.
* Crash tests: `zipshrink-fp.exe` is ended with `TerminateProcess` — no
  cleanup, like a crash or a forced shutdown — at 17 named points (planning,
  opening, writing, flushing, before and after the journal commit, before,
  during and after a deletion, writing the central directory, before and after
  the rename, cleanup) and with a torn journal write; then the job is resumed,
  including crashes during the resume itself. Disk-full and I/O errors are
  injected; journals and partial archives are damaged by hand.
* After every crash the tests assert that **every original is either still
  present unchanged or held, byte for byte, in the archive or the partial
  archive**, and after every resume that extracting the archive with Python's
  `zipfile` reproduces every original's SHA-256 exactly. "The ZIP opens" is
  never enough.
* CI runs the suite natively on Windows and under Wine in Windows 10 and
  Windows 95 modes.

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
