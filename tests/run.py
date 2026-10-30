#!/usr/bin/env python3
"""ZipShrink test suite.

Runs the Windows executables natively on Windows, or under Wine elsewhere,
on generated file trees, and checks every result independently with
Python's zipfile module and SHA-256 of every entry against the originals.

    python3 tests/run.py --build build                 # all tests
    python3 tests/run.py --build build -k crash        # tests whose name matches
    python3 tests/run.py --build build --win95         # Wine in Windows 95 mode
    python3 tests/run.py --build build --slow          # also the >4 GiB test

The crash tests use zipshrink-fp.exe, built with fault injection: the
environment variable ZIPSHRINK_FAILPOINT ends the process at a chosen point
(exit code 70, like a power cut or a killed process), or makes an operation
fail with "disk full" or an I/O error. After every crash the test checks that
no original is lost, resumes the job, and checks the finished archive.
"""
import argparse
import hashlib
import os
import random
import shutil
import stat
import subprocess
import sys
import tempfile
import time
import traceback
import zipfile

FP_EXIT = 70
EX_OK, EX_ERROR, EX_USAGE, EX_ISSUES, EX_NO_SPACE, EX_INTERRUPTED, EX_VERIFY, EX_DAMAGED = range(8)

# Every failpoint in the engine, in the order a job reaches them.
FAILPOINTS = [
    "before_plan_done",
    "open_source",
    "archive_write",
    "entry_header_written",
    "entry_data_written",
    "before_archive_sync",
    "batch_synced",
    "before_journal_commit",
    "journal_committed",
    "before_delete",
    "delete_source",
    "after_delete",
    "finalize_cd_partial",
    "finalize_written",
    "before_rename",
    "after_rename",
    "before_cleanup",
]

ARGS = None
TESTS = []


def test(fn):
    TESTS.append(fn)
    return fn


class Fail(Exception):
    pass


def check(cond, msg):
    if not cond:
        raise Fail(msg)


# ---------- running the programs ----------

def exe(name):
    return os.path.join(ARGS.build, name)


def run(name, *args, cwd, env=None, stdin=None, timeout=600):
    cmd = [exe(name)] + list(args)
    if os.name != "nt":
        cmd = ["wine"] + cmd
    e = dict(os.environ)
    e.pop("ZIPSHRINK_FAILPOINT", None)
    if env:
        e.update(env)
    if os.name != "nt":
        e["WINEDEBUG"] = "-all"
        if ARGS.prefix:
            e["WINEPREFIX"] = ARGS.prefix
    p = subprocess.run(cmd, cwd=cwd, env=e, input=stdin, capture_output=True, timeout=timeout)
    out = p.stdout.decode("utf-8", "replace")
    err = "\n".join(l for l in p.stderr.decode("utf-8", "replace").splitlines() if not l.startswith("wine:"))
    return p.returncode, out, err


def zs(*args, cwd, fp=None, expect=None, **kw):
    env = {"ZIPSHRINK_FAILPOINT": fp} if fp else None
    code, out, err = run("zipshrink-fp.exe" if fp else "zipshrink.exe", *args, cwd=cwd, env=env, **kw)
    if expect is not None:
        allowed = expect if isinstance(expect, (list, tuple, set)) else [expect]
        if code not in allowed:
            raise Fail(f"zipshrink {' '.join(args)} (failpoint {fp}) exited {code}, expected {allowed}\n--- stdout\n{out}\n--- stderr\n{err}")
    return code, out, err


# ---------- file trees ----------

def sha(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def make_tree(root, seed=1, unicode_names=True, readonly=False, many=60):
    """A varied tree: empty, tiny, compressible, incompressible, nested,
    empty folder, names with spaces and accents."""
    rnd = random.Random(seed)
    os.makedirs(root)

    def put(rel, data):
        p = os.path.join(root, rel)
        os.makedirs(os.path.dirname(p), exist_ok=True)
        with open(p, "wb") as f:
            f.write(data)

    put("empty.txt", b"")
    put("tiny.txt", b"x")
    put("text/lorem.txt", (b"Lorem ipsum dolor sit amet, consectetur adipiscing elit. " * 40000))
    put("bin/random.bin", rnd.randbytes(700_000))
    put("bin/mixed.bin", b"".join(rnd.randbytes(50) + b"\0" * 200 for _ in range(4000)))
    put("deep/a/b/c/d/leaf.txt", b"leaf\n" * 1000)
    put("with space/file name.txt", b"spaces\n")
    os.makedirs(os.path.join(root, "empty folder"))
    for i in range(many):
        put(f"many/f{i:03}.dat", rnd.randbytes(rnd.randrange(0, 5000)))
    if unicode_names:
        put("accents/café naïve.txt", "café\n".encode())
    if readonly:
        p = os.path.join(root, "readonly.txt")
        put("readonly.txt", b"read only\n")
        os.chmod(p, stat.S_IREAD)


def snapshot(root):
    """{archive name: sha256} for files and {archive name} for folders, with
    the root folder's name as the top level, as ZipShrink stores them."""
    top = os.path.basename(os.path.normpath(root))
    files, dirs = {}, {top + "/"}
    for d, subdirs, names in os.walk(root):
        rel = os.path.relpath(d, root).replace(os.sep, "/")
        base = top if rel == "." else f"{top}/{rel}"
        for s in subdirs:
            dirs.add(f"{base}/{s}/")
        for n in names:
            files[f"{base}/{n}"] = sha(os.path.join(d, n))
    return files, dirs


def zip_contents(path):
    files, dirs = {}, set()
    with zipfile.ZipFile(path) as z:
        check(z.testzip() is None, f"{path}: zipfile reports a bad CRC")
        for i in z.infolist():
            if i.filename.endswith("/"):
                dirs.add(i.filename)
            else:
                h = hashlib.sha256()
                with z.open(i) as f:
                    for chunk in iter(lambda: f.read(1 << 20), b""):
                        h.update(chunk)
                check(i.filename not in files, f"{path}: duplicate entry {i.filename}")
                files[i.filename] = h.hexdigest()
    return files, dirs


def expect_zip(path, files, dirs=None, exact=True):
    got_f, got_d = zip_contents(path)
    for n, h in files.items():
        check(n in got_f, f"{path}: {n} is missing")
        check(got_f[n] == h, f"{path}: {n} has the wrong content")
    if exact:
        extra = set(got_f) - set(files)
        check(not extra, f"{path}: unexpected entries {sorted(extra)[:5]}")
    if dirs is not None:
        missing = set(dirs) - got_d
        check(not missing, f"{path}: folders missing {sorted(missing)[:5]}")
    return got_f


def remaining_files(root):
    if not os.path.exists(root):
        return {}
    return snapshot(root)[0]


class Case:
    """A temporary folder for one test."""

    def __init__(self, name):
        self.dir = tempfile.mkdtemp(prefix=f"zs-{name}-", dir=ARGS.tmp)

    def path(self, *p):
        return os.path.join(self.dir, *p)

    def cleanup(self):
        for d, _, names in os.walk(self.dir):
            for n in names:
                try:
                    os.chmod(os.path.join(d, n), stat.S_IWRITE | stat.S_IREAD)
                except OSError:
                    pass
        shutil.rmtree(self.dir, ignore_errors=True)


def no_job_files(c, name="out.zip"):
    for suffix in (".partial", ".partial.journal"):
        check(not os.path.exists(c.path(name + suffix)), f"{name}{suffix} was left behind")


# ---------- tests ----------

@test
def roundtrip_keep_originals(c):
    make_tree(c.path("src"))
    files, dirs = snapshot(c.path("src"))
    zs("create", "src", "-o", "out.zip", "--min-free", "16M", cwd=c.dir, expect=EX_OK)
    expect_zip(c.path("out.zip"), files, dirs)
    check(remaining_files(c.path("src")) == files, "originals changed although deletion was off")
    no_job_files(c)
    code, out, _ = zs("verify", "out.zip", cwd=c.dir, expect=EX_OK)
    check("matched their stored SHA-256" in out, "verify did not report SHA-256 checks")
    zs("verify", "out.zip", "--quick", cwd=c.dir, expect=EX_OK)


@test
def roundtrip_levels(c):
    make_tree(c.path("src"), seed=2, many=10)
    files, dirs = snapshot(c.path("src"))
    for level in ("0", "1", "9"):
        zs("create", "src", "-o", f"l{level}.zip", "-l", level, cwd=c.dir, expect=EX_OK)
        expect_zip(c.path(f"l{level}.zip"), files, dirs)
    check(os.path.getsize(c.path("l9.zip")) < os.path.getsize(c.path("l0.zip")), "level 9 is not smaller than level 0")


@test
def delete_originals(c):
    make_tree(c.path("src"), seed=3)
    files, dirs = snapshot(c.path("src"))
    _, out, _ = zs("create", "src", "-o", "out.zip", "--delete-originals", "-y", cwd=c.dir, expect=EX_OK)
    expect_zip(c.path("out.zip"), files, dirs)
    check(not os.path.exists(c.path("src")), "the emptied source folder was not removed")
    check("deleted" in out, "the summary does not mention deletion")
    no_job_files(c)


@test
def delete_needs_confirmation(c):
    make_tree(c.path("src"), seed=4, many=3)
    files, _ = snapshot(c.path("src"))
    # Not a terminal: the explicit flag is taken as consent (with a warning).
    _, _, err = zs("create", "src", "-o", "out.zip", "--delete-originals", cwd=c.dir, stdin=b"no\n", expect=EX_OK)
    check("not a terminal" in err, "no warning about proceeding without confirmation")
    expect_zip(c.path("out.zip"), files)


@test
def readonly_files_are_kept(c):
    make_tree(c.path("src"), seed=5, many=5, readonly=True)
    files, dirs = snapshot(c.path("src"))
    _, _, err = zs("create", "src", "-o", "out.zip", "--delete-originals", "-y", cwd=c.dir, expect=EX_ISSUES)
    expect_zip(c.path("out.zip"), files, dirs)
    left = remaining_files(c.path("src"))
    check(list(left) == ["src/readonly.txt"], f"expected only the read-only file to remain, found {sorted(left)}")
    check("read-only" in err, "the report does not say why the file was kept")


@test
def readonly_files_deleted_when_asked(c):
    make_tree(c.path("src"), seed=5, many=5, readonly=True)
    files, _ = snapshot(c.path("src"))
    zs("create", "src", "-o", "out.zip", "--delete-originals", "--delete-readonly", "-y", cwd=c.dir, expect=EX_OK)
    expect_zip(c.path("out.zip"), files)
    check(not os.path.exists(c.path("src")), "files were left")


@test
def quick_change_check(c):
    make_tree(c.path("src"), seed=6, many=5)
    files, _ = snapshot(c.path("src"))
    zs("create", "src", "-o", "out.zip", "--delete-originals", "--quick-change-check", "-y", cwd=c.dir, expect=EX_OK)
    expect_zip(c.path("out.zip"), files)


@test
def several_sources_and_single_file(c):
    make_tree(c.path("a"), seed=7, many=3)
    make_tree(c.path("b"), seed=8, many=3)
    with open(c.path("single.txt"), "wb") as f:
        f.write(b"single\n" * 100)
    fa, _ = snapshot(c.path("a"))
    fb, _ = snapshot(c.path("b"))
    want = {**fa, **fb, "single.txt": sha(c.path("single.txt"))}
    zs("create", "a", "b", "single.txt", "-o", "out.zip", "--delete-originals", "-y", cwd=c.dir, expect=EX_OK)
    expect_zip(c.path("out.zip"), want)
    check(not os.path.exists(c.path("single.txt")), "the single file was not deleted")


@test
def zip64_forced(c):
    make_tree(c.path("src"), seed=9, many=5)
    files, dirs = snapshot(c.path("src"))
    zs("create", "src", "-o", "out.zip", "--force-zip64", cwd=c.dir, expect=EX_OK)
    expect_zip(c.path("out.zip"), files, dirs)
    zs("verify", "out.zip", cwd=c.dir, expect=EX_OK)


@test
def many_files_batches(c):
    make_tree(c.path("src"), seed=10, many=1500)
    files, dirs = snapshot(c.path("src"))
    zs("create", "src", "-o", "out.zip", "--delete-originals", "-y", "-l", "1", cwd=c.dir, expect=EX_OK)
    expect_zip(c.path("out.zip"), files, dirs)


@test
def split_parts(c):
    make_tree(c.path("src"), seed=11, many=40)
    files, _ = snapshot(c.path("src"))
    _, out, _ = zs("check", "src", "-o", "out.zip", "--parts", "3", cwd=c.dir, expect=EX_OK)
    check("out.part1.zip" in out and "out.part3.zip" in out, "check does not preview the parts")
    zs("create", "src", "-o", "out.zip", "--parts", "3", "--delete-originals", "-y", cwd=c.dir, expect=EX_OK)
    union = {}
    for k in (1, 2, 3):
        p = c.path(f"out.part{k}.zip")
        check(os.path.exists(p), f"part {k} is missing")
        got, _ = zip_contents(p)
        check(got, f"part {k} is empty")
        for n in got:
            check(n not in union, f"{n} is in more than one part")
        union.update(got)
        zs("verify", f"out.part{k}.zip", cwd=c.dir, expect=EX_OK)
    check(union == files, "the parts together do not hold exactly the originals")
    check(not os.path.exists(c.path("src")), "originals were left")


@test
def split_crash_and_resume(c):
    make_tree(c.path("src"), seed=12, many=40)
    files, _ = snapshot(c.path("src"))
    # The 2nd part's first commit crashes; resume continues it and part 3.
    code, _, _ = zs("create", "src", "-o", "out.zip", "--parts", "3", "--delete-originals", "-y", "--batch-entries", "4",
                    cwd=c.dir, fp="journal_committed:8", expect=[FP_EXIT])
    check_no_loss(c, files, ["out.part1.zip", "out.part2.zip", "out.part3.zip"])
    zs("resume", "out.part2.zip.partial", "-y", cwd=c.dir, expect=EX_OK)
    union = {}
    for k in (1, 2, 3):
        got, _ = zip_contents(c.path(f"out.part{k}.zip"))
        union.update(got)
        no_job_files(c, f"out.part{k}.zip")
    check(union == files, "after resume the parts do not hold exactly the originals")


@test
def refuses_existing_output(c):
    make_tree(c.path("src"), seed=13, many=2)
    files, _ = snapshot(c.path("src"))
    with open(c.path("out.zip"), "wb") as f:
        f.write(b"precious")
    zs("create", "src", "-o", "out.zip", cwd=c.dir, expect=EX_ERROR)
    check(open(c.path("out.zip"), "rb").read() == b"precious", "the existing file was touched")
    zs("create", "src", "-o", "out.zip", "--overwrite", cwd=c.dir, expect=EX_OK)
    expect_zip(c.path("out.zip"), files)


@test
def refuses_output_inside_source(c):
    make_tree(c.path("src"), seed=14, many=2)
    zs("create", "src", "-o", os.path.join("src", "out.zip"), "--delete-originals", "-y", cwd=c.dir, expect=EX_ERROR)
    check(not os.path.exists(c.path("src", "out.zip")), "an archive was created inside the source")


@test
def bad_command_lines(c):
    zs("create", cwd=c.dir, expect=EX_USAGE)
    zs("create", "x", cwd=c.dir, expect=EX_USAGE)
    zs("create", "x", "-o", "y.zip", "--level", "12", cwd=c.dir, expect=EX_USAGE)
    zs("create", "x", "-o", "y.zip", "--frobnicate", cwd=c.dir, expect=EX_USAGE)
    zs("frobnicate", cwd=c.dir, expect=EX_USAGE)
    zs("create", "x", "-o", "y.zip", "--delete-readonly", cwd=c.dir, expect=EX_USAGE)
    zs("create", "missing", "-o", "y.zip", cwd=c.dir, expect=EX_ERROR)
    code, out, _ = zs("--help", cwd=c.dir, expect=EX_OK)
    check("Exit codes" in out, "help text is missing")


@test
def space_check(c):
    make_tree(c.path("src"), seed=15, many=2)
    _, out, _ = zs("check", "src", "-o", "out.zip", cwd=c.dir, expect=EX_OK)
    check("It fits" in out, f"unexpected verdict:\n{out}")
    zs("check", "src", "-o", "out.zip", "--min-free", "900000G", cwd=c.dir, expect=EX_NO_SPACE)
    zs("create", "src", "-o", "out.zip", "--min-free", "900000G", cwd=c.dir, expect=EX_NO_SPACE)
    check(not os.path.exists(c.path("out.zip.partial")), "a job was started although it cannot fit")


def check_no_loss(c, files, archives):
    """Every original is either still on disk unchanged, or in an archive or
    partial archive (committed or not, the bytes must be there)."""
    left = remaining_files(c.path("src"))
    for n, h in left.items():
        check(files.get(n) == h, f"original {n} changed")
    gone = set(files) - set(left)
    if not gone:
        return
    stored = {}
    for a in archives:
        for p in (c.path(a), c.path(a + ".partial")):
            if os.path.exists(p):
                stored.update(scan_local_entries(p))
    for n in gone:
        check(n in stored, f"{n} was deleted but is not in any archive")
        check(stored[n] == files[n], f"{n} was deleted but its archived copy differs")


def scan_local_entries(path):
    """Reads entries by their local headers (a .partial has no central
    directory yet); returns {name: sha256} of those that decompress."""
    import struct
    import zlib
    data = open(path, "rb").read()
    out, pos = {}, 0
    while True:
        pos = data.find(b"PK\x03\x04", pos)
        if pos < 0 or pos + 30 > len(data):
            return out
        (_, _, flags, method, _, _, crc, cs, us, nl, xl) = struct.unpack("<IHHHHHIIIHH", data[pos:pos + 30])
        name = data[pos + 30:pos + 30 + nl].decode("utf-8", "replace")
        extra = data[pos + 30 + nl:pos + 30 + nl + xl]
        if cs == 0xFFFFFFFF or us == 0xFFFFFFFF:
            i = 0
            while i + 4 <= len(extra):
                tag, size = struct.unpack("<HH", extra[i:i + 4])
                if tag == 1:
                    us, cs = struct.unpack("<QQ", extra[i + 4:i + 20])
                i += 4 + size
        start = pos + 30 + nl + xl
        body = data[start:start + cs]
        pos += 4
        try:
            raw = body if method == 0 else zlib.decompressobj(-15).decompress(body)
        except zlib.error:
            continue
        if len(raw) == us and zlib.crc32(raw) & 0xFFFFFFFF == crc and not name.endswith("/"):
            out[name] = hashlib.sha256(raw).hexdigest()


def crash_and_resume(c, fp, seed, batch="3", extra=(), many=25):
    make_tree(c.path("src"), seed=seed, many=many)
    files, dirs = snapshot(c.path("src"))
    code, out, err = zs("create", "src", "-o", "out.zip", "--delete-originals", "-y", "--batch-entries", batch, *extra,
                        cwd=c.dir, fp=fp, expect=[FP_EXIT, EX_OK, EX_ISSUES, EX_NO_SPACE, EX_INTERRUPTED])
    check_no_loss(c, files, ["out.zip"])
    if code == FP_EXIT or os.path.exists(c.path("out.zip.partial.journal")):
        zs("status", "out.zip", cwd=c.dir, expect=EX_OK)
        zs("resume", "out.zip", "-y", cwd=c.dir, expect=[EX_OK, EX_ISSUES])
    expect_zip(c.path("out.zip"), files, dirs)
    no_job_files(c)
    check(not remaining_files(c.path("src")), f"originals were left after resume: {sorted(remaining_files(c.path('src')))[:5]}")
    return code


def make_crash_test(fp, nth):
    def t(c):
        code = crash_and_resume(c, f"{fp}:{nth}", seed=100 + nth)
        check(code == FP_EXIT, f"failpoint {fp}:{nth} was never reached (exit {code})")
    t.__name__ = f"crash_{fp}_{nth}"
    return t


for _fp in FAILPOINTS:
    for _n in ((1, 4) if _fp not in ("before_plan_done", "finalize_cd_partial", "finalize_written", "before_rename", "after_rename", "before_cleanup") else (1,)):
        TESTS.append(make_crash_test(_fp, _n))


@test
def crash_torn_journal_record(c):
    code = crash_and_resume(c, "journal_torn:3", seed=200)
    check(code == FP_EXIT, "the torn-record failpoint was not reached")


@test
def crash_while_resuming(c):
    make_tree(c.path("src"), seed=201, many=30)
    files, dirs = snapshot(c.path("src"))
    zs("create", "src", "-o", "out.zip", "--delete-originals", "-y", "--batch-entries", "3", cwd=c.dir, fp="journal_committed:3", expect=FP_EXIT)
    check_no_loss(c, files, ["out.zip"])
    zs("resume", "out.zip", "-y", cwd=c.dir, fp="before_delete:5", expect=FP_EXIT)
    check_no_loss(c, files, ["out.zip"])
    zs("resume", "out.zip.partial.journal", "-y", cwd=c.dir, fp="entry_data_written:2", expect=FP_EXIT)
    check_no_loss(c, files, ["out.zip"])
    zs("resume", "out.zip.partial", "-y", cwd=c.dir, expect=EX_OK)
    expect_zip(c.path("out.zip"), files, dirs)
    no_job_files(c)


@test
def resume_without_deleting(c):
    make_tree(c.path("src"), seed=202, many=20)
    files, _ = snapshot(c.path("src"))
    zs("create", "src", "-o", "out.zip", "--delete-originals", "-y", "--batch-entries", "3", cwd=c.dir, fp="journal_committed:2", expect=FP_EXIT)
    before = remaining_files(c.path("src"))
    zs("resume", "out.zip", "--no-delete", cwd=c.dir, expect=EX_OK)
    expect_zip(c.path("out.zip"), files)
    after = remaining_files(c.path("src"))
    check(set(after) == set(before), "--no-delete still deleted originals")


@test
def disk_full_pauses_and_resumes(c):
    make_tree(c.path("src"), seed=203, many=20)
    files, dirs = snapshot(c.path("src"))
    code, _, err = zs("create", "src", "-o", "out.zip", "--delete-originals", "-y", "--batch-entries", "3", cwd=c.dir,
                      fp="archive_write:12+=enospc", expect=EX_NO_SPACE)
    check("resume" in err, "the message does not say how to continue")
    check_no_loss(c, files, ["out.zip"])
    zs("resume", "out.zip", "-y", cwd=c.dir, expect=EX_OK)
    expect_zip(c.path("out.zip"), files, dirs)


@test
def unreadable_file_is_kept(c):
    make_tree(c.path("src"), seed=204, many=5)
    files, _ = snapshot(c.path("src"))
    _, _, err = zs("create", "src", "-o", "out.zip", "--delete-originals", "-y", cwd=c.dir, fp="open_source:3+=eio", expect=EX_ISSUES)
    got, _ = zip_contents(c.path("out.zip"))
    left = remaining_files(c.path("src"))
    check(left, "files that could not be read were deleted")
    for n in left:
        check(n not in got, f"{n} is both kept and archived")
    check(set(got) | set(left) == set(files), "files are missing from both the archive and the disk")
    for n, h in got.items():
        check(files[n] == h, f"{n} differs")
    check("not archived" in err, "the report does not list the failed files")


@test
def verify_detects_corruption(c):
    make_tree(c.path("src"), seed=205, many=3)
    zs("create", "src", "-o", "out.zip", "-l", "0", cwd=c.dir, expect=EX_OK)
    data = bytearray(open(c.path("out.zip"), "rb").read())
    i = data.find(b"Lorem ipsum")
    check(i > 0, "test data not found")
    data[i + 5000] ^= 0x20
    open(c.path("bad.zip"), "wb").write(data)
    _, _, err = zs("verify", "bad.zip", cwd=c.dir, expect=EX_VERIFY)
    check("lorem.txt" in err, f"the damaged entry is not named:\n{err}")
    zs("verify", "bad.zip", "--quick", cwd=c.dir, expect=EX_OK)
    open(c.path("trunc.zip"), "wb").write(data[: len(data) // 2])
    zs("verify", "trunc.zip", cwd=c.dir, expect=EX_VERIFY)


@test
def salvage_without_journal(c):
    make_tree(c.path("src"), seed=206, many=20)
    files, _ = snapshot(c.path("src"))
    zs("create", "src", "-o", "out.zip", "--batch-entries", "3", cwd=c.dir, fp="entry_data_written:15", expect=FP_EXIT)
    os.remove(c.path("out.zip.partial.journal"))
    _, out, _ = zs("salvage", "out.zip.partial", cwd=c.dir, expect=EX_OK)
    got = expect_zip(c.path("out.zip"), {}, exact=False)
    check(len(got) >= 10, f"salvage recovered only {len(got)} files")
    for n, h in got.items():
        check(files[n] == h, f"salvaged {n} differs")
    zs("verify", "out.zip", cwd=c.dir, expect=EX_OK)


@test
def damaged_journal_stops(c):
    make_tree(c.path("src"), seed=207, many=20)
    files, _ = snapshot(c.path("src"))
    zs("create", "src", "-o", "out.zip", "--delete-originals", "-y", "--batch-entries", "3", cwd=c.dir, fp="journal_committed:3", expect=FP_EXIT)
    j = bytearray(open(c.path("out.zip.partial.journal"), "rb").read())
    j[len(j) // 2] ^= 0x01
    open(c.path("out.zip.partial.journal"), "wb").write(j)
    before = remaining_files(c.path("src"))
    zs("resume", "out.zip", "-y", cwd=c.dir, expect=EX_DAMAGED)
    zs("status", "out.zip", cwd=c.dir, expect=EX_DAMAGED)
    check(remaining_files(c.path("src")) == before, "originals were deleted with a damaged journal")
    zs("salvage", "out.zip.partial", "-o", "rescued.zip", cwd=c.dir, expect=EX_OK)
    check_no_loss(c, files, ["rescued.zip"])


@test
def truncated_partial_stops(c):
    make_tree(c.path("src"), seed=208, many=20)
    zs("create", "src", "-o", "out.zip", "--delete-originals", "-y", "--batch-entries", "3", cwd=c.dir, fp="journal_committed:4", expect=FP_EXIT)
    size = os.path.getsize(c.path("out.zip.partial"))
    with open(c.path("out.zip.partial"), "r+b") as f:
        f.truncate(size // 3)
    before = remaining_files(c.path("src"))
    zs("resume", "out.zip", "-y", cwd=c.dir, expect=EX_DAMAGED)
    check(remaining_files(c.path("src")) == before, "originals were deleted after the archive was damaged")


@test
def changed_partial_stops(c):
    make_tree(c.path("src"), seed=209, many=20)
    zs("create", "src", "-o", "out.zip", "--delete-originals", "-y", "--batch-entries", "3", cwd=c.dir, fp="journal_committed:4", expect=FP_EXIT)
    data = bytearray(open(c.path("out.zip.partial"), "rb").read())
    data[40] ^= 0xFF  # inside the first local header's name or fields
    open(c.path("out.zip.partial"), "wb").write(data)
    zs("resume", "out.zip", "-y", cwd=c.dir, expect=EX_DAMAGED)


@test
def file_appearing_after_scan_is_left_alone(c):
    make_tree(c.path("src"), seed=210, many=5)
    files, _ = snapshot(c.path("src"))
    zs("create", "src", "-o", "out.zip", "--delete-originals", "-y", "--batch-entries", "2", cwd=c.dir, fp="journal_committed:1", expect=FP_EXIT)
    with open(c.path("src", "new.txt"), "wb") as f:
        f.write(b"created during the job\n")
    zs("resume", "out.zip", "-y", cwd=c.dir, expect=[EX_OK, EX_ISSUES])
    check(os.path.exists(c.path("src", "new.txt")), "a file that was not in the plan was deleted")
    expect_zip(c.path("out.zip"), files)


@test
def unit_tests(c):
    code, out, err = run("zs-tests.exe", cwd=c.dir)
    check(code == 0, f"zs-tests.exe failed ({code}):\n{out}\n{err}")


@test
def large_file_zip64(c):
    if not ARGS.slow:
        return "skipped (use --slow)"
    os.makedirs(c.path("src"))
    with open(c.path("src", "big.bin"), "wb") as f:
        f.truncate((4 << 30) + 12345)
    files, _ = snapshot(c.path("src"))
    zs("create", "src", "-o", "out.zip", "-l", "1", "--delete-originals", "-y", cwd=c.dir, expect=EX_OK, timeout=7200)
    expect_zip(c.path("out.zip"), files)


@test
def non_ascii_names(c):
    os.makedirs(c.path("src"))
    names = ["été.txt", "日本語.txt", "Док.txt"]
    for n in names:
        with open(c.path("src", n), "wb") as f:
            f.write(n.encode() * 10)
    files, _ = snapshot(c.path("src"))
    code, _, err = zs("create", "src", "-o", "out.zip", "--delete-originals", "-y", cwd=c.dir, expect=[EX_OK, EX_ISSUES])
    got, _ = zip_contents(c.path("out.zip"))
    left = remaining_files(c.path("src"))
    for n, h in got.items():
        check(files[n] == h, f"{n} differs")
    if ARGS.win95:
        # The ANSI API cannot name files outside the system code page: they
        # are skipped and left in place, never guessed at.
        check(code == EX_ISSUES and left, "unrepresentable names were not skipped on Windows 95")
        check(set(got) | set(left) == set(files), "files are missing")
    else:
        check(code == EX_OK and set(got) == set(files), "Unicode names were not all archived")


def main():
    global ARGS
    ap = argparse.ArgumentParser()
    ap.add_argument("--build", default="build")
    ap.add_argument("-k", dest="pattern", default="")
    ap.add_argument("--slow", action="store_true")
    ap.add_argument("--win95", action="store_true", help="Wine prefix set to Windows 95 (WINEPREFIX or --prefix)")
    ap.add_argument("--prefix", default=os.environ.get("WINEPREFIX"))
    ap.add_argument("--tmp", default=None, help="folder for test data")
    ap.add_argument("--keep", action="store_true", help="keep the folders of failed tests")
    ARGS = ap.parse_args()
    ARGS.build = os.path.abspath(ARGS.build)
    if ARGS.tmp:
        os.makedirs(ARGS.tmp, exist_ok=True)
    if os.name != "nt":
        code, out, _ = run("zipshrink.exe", "--version", cwd=ARGS.build)
        print(f"Running under Wine: {out.strip()}")
    selected = [t for t in TESTS if ARGS.pattern in t.__name__]
    failed = []
    start = time.time()
    for t in selected:
        c = Case(t.__name__)
        t0 = time.time()
        try:
            note = t(c)
            print(f"ok    {t.__name__} ({time.time() - t0:.1f}s){' ' + note if note else ''}", flush=True)
            c.cleanup()
        except Exception as e:
            failed.append(t.__name__)
            msg = str(e) if isinstance(e, Fail) else traceback.format_exc()
            print(f"FAIL  {t.__name__}: {msg}", flush=True)
            if os.environ.get("GITHUB_ACTIONS"):
                first = msg.splitlines()[0] if msg else ""
                print(f"::error title=test {t.__name__} failed::{first}")
            if ARGS.keep:
                print(f"      kept {c.dir}")
            else:
                c.cleanup()
    print(f"\n{len(selected) - len(failed)} passed, {len(failed)} failed in {time.time() - start:.0f}s")
    if failed:
        print("failed: " + ", ".join(failed))
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
