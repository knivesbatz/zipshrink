/* zipshrink.exe: the command line program. */
#include "engine.h"
#include "journal.h"
#include "update.h"

enum { EX_OK = 0, EX_ERROR = 1, EX_USAGE = 2, EX_ISSUES = 3, EX_NO_SPACE = 4, EX_INTERRUPTED = 5, EX_VERIFY = 6, EX_DAMAGED = 7 };

static const char USAGE[] =
    "ZipShrink " ZS_VERSION " - create ZIP archives that reclaim disk space as they go\n"
    "\n"
    "Usage:\n"
    "  zipshrink create SOURCE... -o ZIP [options]   create an archive\n"
    "  zipshrink check SOURCE... -o ZIP [options]    check whether it would fit\n"
    "  zipshrink resume PATH [options]               continue an interrupted job\n"
    "  zipshrink verify ZIP [--quick]                decompress and check every entry\n"
    "  zipshrink status PATH                         show an unfinished job\n"
    "  zipshrink salvage PARTIAL [-o ZIP]            rebuild an archive without its journal\n"
    "  zipshrink update [--check]                    look for a newer version\n"
    "\n"
    "Options for create and check:\n"
    "  -o, --output ZIP        the ZIP file to create\n"
    "  -l, --level N           0 stores, 1 is fastest, 9 is smallest (default 6)\n"
    "  -p, --parts N           split into N separate ZIP files of about equal size\n"
    "  -s, --part-size SIZE    split into separate ZIP files of about SIZE each\n"
    "                          (such as 650M or 2G); a file is never cut in two\n"
    "      --min-free SIZE     free space to keep on the destination (default 256M)\n"
    "      --delete-originals  permanently delete each original once its archived\n"
    "                          copy is verified on disk\n"
    "  -y, --yes               do not ask before deleting originals\n"
    "      --delete-readonly   also delete read-only files (normally kept)\n"
    "      --quick-change-check  before deleting, compare size, times and file id\n"
    "                          instead of re-reading the whole file\n"
    "      --overwrite         replace ZIP if it exists (once the new one is verified)\n"
    "      --no-final-verify   skip decompressing everything again at the end\n"
    "      --ignore-space-check  start even if the archive may not fit\n"
    "  -q, --quiet             print only the summary and problems\n"
    "  -v, --verbose           list every file as it is archived and deleted\n"
    "\n"
    "Options for resume: --no-delete, -y, --min-free SIZE, -q, -v\n"
    "\n"
    "Exit codes:\n"
    "  0 success                       4 not enough disk space\n"
    "  1 error                         5 interrupted; run zipshrink resume\n"
    "  2 invalid command line          6 verification failed\n"
    "  3 finished with skipped, failed 7 the partial archive or journal is\n"
    "    or kept files                   damaged; see zipshrink salvage\n"
    "\n"
    "Originals are never deleted unless --delete-originals is given.\n";

static volatile LONG g_cancel;
static volatile LONG g_presses;

static BOOL WINAPI on_ctrl(DWORD type) {
    if (type != CTRL_C_EVENT && type != CTRL_BREAK_EVENT) return FALSE;
    /* Plain stores: Windows 95 runs on a 386, which has no atomic increment. */
    if (++g_presses == 1) {
        g_cancel = 1;
        zs_err("\nStopping safely at the next safe point... (press Ctrl+C again to quit now; that is safe too)\n");
    } else {
        zs_err("\nQuitting. Run zipshrink resume to continue.\n");
        ExitProcess(EX_INTERRUPTED);
    }
    return TRUE;
}

static void err(const char *msg) { zs_errf("error: %s\n", msg); }
static void warn(const char *msg) { zs_errf("warning: %s\n", msg); }

static int is_console(DWORD which) {
    HANDLE h = GetStdHandle(which);
    DWORD mode;
    return h && h != INVALID_HANDLE_VALUE && GetFileType(h) == FILE_TYPE_CHAR && GetConsoleMode(h, &mode);
}

static int code_for(ErrKind k) {
    switch (k) {
    case ERR_SPACE: return EX_NO_SPACE;
    case ERR_DAMAGED: return EX_DAMAGED;
    default: return EX_ERROR;
    }
}

/* "12.3" for a/b as a percentage. */
static char *percent(u64 a, u64 b) {
    u64 t;
    if (!b) return zs_strdup("0");
    t = a > 0xFFFFFFFFFFFFULL ? a / (b / 1000 + 1) : a * 1000 / b;
    return zs_fmt("%U.%U", t / 10, t % 10);
}

static void print_human(const char *fmt, u64 n) {
    char *h = zs_human(n);
    zs_outf(fmt, h);
    zs_free(h);
}

/* ---------- argument parsing ---------- */

typedef struct {
    PtrVec pos;
    const char *output;
    int level, parts;
    u64 min_free, part_size;
    int has_min_free;
    int del, yes, delro, quick_change, overwrite, no_final, ignore_space, quiet, verbose, no_delete, quick, check;
    u32 batch;
    int force_zip64;
} Args;

static int need_value(int argc, char **argv, int *i, const char **out) {
    if (*i + 1 >= argc) {
        zs_errf("error: %s needs a value\n", argv[*i]);
        return 0;
    }
    *out = argv[++*i];
    return 1;
}

static int parse_int(const char *opt, const char *s, int lo, int hi, int *out) {
    u64 v;
    if (!zs_parse_u64(s, &v) || v < (u64)lo || v > (u64)hi) {
        zs_errf("error: %s must be a number from %d to %d\n", opt, lo, hi);
        return 0;
    }
    *out = (int)v;
    return 1;
}

static int parse_args(int argc, char **argv, int first, Args *a) {
    int i, only_pos = 0;
    memset(a, 0, sizeof(*a));
    a->level = 6, a->parts = 1, a->min_free = DEFAULT_MIN_FREE;
    for (i = first; i < argc; i++) {
        const char *s = argv[i], *v;
        if (only_pos || s[0] != '-' || !s[1]) {
            pv_push(&a->pos, (void *)s);
            continue;
        }
        if (!zs_strcmp(s, "--")) only_pos = 1;
        else if (!zs_strcmp(s, "-o") || !zs_strcmp(s, "--output")) {
            if (!need_value(argc, argv, &i, &a->output)) return 0;
        } else if (!zs_strcmp(s, "-l") || !zs_strcmp(s, "--level")) {
            if (!need_value(argc, argv, &i, &v) || !parse_int("--level", v, 0, 9, &a->level)) return 0;
        } else if (!zs_strcmp(s, "-p") || !zs_strcmp(s, "--parts")) {
            if (!need_value(argc, argv, &i, &v) || !parse_int("--parts", v, 1, ZS_MAX_PARTS, &a->parts)) return 0;
        } else if (!zs_strcmp(s, "-s") || !zs_strcmp(s, "--part-size")) {
            if (!need_value(argc, argv, &i, &v)) return 0;
            if (!zs_parse_size(v, &a->part_size)) return zs_errf("error: --part-size: %s is not a size such as 650M or 2G\n", v), 0;
            if (a->part_size < ZS_MIN_PART_SIZE) return err("--part-size should be at least 64K"), 0;
        } else if (!zs_strcmp(s, "--min-free")) {
            if (!need_value(argc, argv, &i, &v)) return 0;
            if (!zs_parse_size(v, &a->min_free)) return zs_errf("error: --min-free: %s is not a size such as 512M or 2G\n", v), 0;
            a->has_min_free = 1;
        } else if (!zs_strcmp(s, "--batch-entries")) {
            int n;
            if (!need_value(argc, argv, &i, &v) || !parse_int("--batch-entries", v, 0, 1000000, &n)) return 0;
            a->batch = (u32)n;
        } else if (!zs_strcmp(s, "--force-zip64")) a->force_zip64 = 1;
        else if (!zs_strcmp(s, "--delete-originals")) a->del = 1;
        else if (!zs_strcmp(s, "-y") || !zs_strcmp(s, "--yes")) a->yes = 1;
        else if (!zs_strcmp(s, "--delete-readonly")) a->delro = 1;
        else if (!zs_strcmp(s, "--quick-change-check")) a->quick_change = 1;
        else if (!zs_strcmp(s, "--overwrite")) a->overwrite = 1;
        else if (!zs_strcmp(s, "--no-final-verify")) a->no_final = 1;
        else if (!zs_strcmp(s, "--ignore-space-check")) a->ignore_space = 1;
        else if (!zs_strcmp(s, "-q") || !zs_strcmp(s, "--quiet")) a->quiet = 1;
        else if (!zs_strcmp(s, "-v") || !zs_strcmp(s, "--verbose")) a->verbose = 1;
        else if (!zs_strcmp(s, "--no-delete")) a->no_delete = 1;
        else if (!zs_strcmp(s, "--quick")) a->quick = 1;
        else if (!zs_strcmp(s, "--check")) a->check = 1;
        else if (!zs_strcmp(s, "-j") || !zs_strcmp(s, "--threads")) {
            if (!need_value(argc, argv, &i, &v)) return 0; /* accepted for compatibility */
        } else {
            zs_errf("error: unknown option %s (see zipshrink --help)\n", s);
            return 0;
        }
    }
    if (a->part_size && a->parts > 1) {
        err("use either --parts or --part-size, not both");
        return 0;
    }
    if ((a->delro || a->quick_change) && !a->del) {
        err("--delete-readonly and --quick-change-check need --delete-originals");
        return 0;
    }
    return 1;
}

/* ---------- confirmation ---------- */

static int read_line(char *buf, size_t cap) {
    HANDLE h = GetStdHandle(STD_INPUT_HANDLE);
    size_t n = 0;
    for (;;) {
        char c;
        DWORD got = 0;
        if (!ReadFile(h, &c, 1, &got, NULL) || !got) break;
        if (c == '\n') break;
        if (c != '\r' && n + 1 < cap) buf[n++] = c;
    }
    buf[n] = 0;
    return 1;
}

static int confirm_deletion(const char *what) {
    char line[64];
    const char *p;
    zs_err("\n  *** PERMANENT DELETION ***\n");
    zs_errf("Each original file in %s will be permanently deleted (not moved to the\n", what);
    zs_err("Recycle Bin) right after its compressed copy is verified on disk.\n");
    zs_err("Files that change, cannot be verified or are read-only are kept.\n");
    if (!is_console(STD_INPUT_HANDLE)) {
        warn("standard input is not a terminal; proceeding because --delete-originals was given");
        return 1;
    }
    zs_err("Type delete to continue: ");
    read_line(line, sizeof(line));
    p = line;
    while (*p == ' ' || *p == '\t') p++;
    {
        size_t n = strlen(p);
        while (n && (p[n - 1] == ' ' || p[n - 1] == '\t')) n--;
        char *t = zs_strndup(p, n);
        int ok = !zs_stricmp(t, "delete");
        zs_free(t);
        return ok;
    }
}

static char *describe_sources(size_t n, const char *first) {
    return n == 1 ? zs_fmt("\"%s\"", first) : zs_fmt("the %U selected items", (u64)n);
}

/* ---------- preflight ---------- */

static void print_preflight(const Preflight *p, const Scan *s) {
    char *pct = percent(p->ratio_permille, 1000);
    char *a = zs_human(p->bytes), *b = zs_human(p->estimated), *c = zs_human(p->worst), *f = zs_human(p->free_space), *m = zs_human(p->min_free);
    size_t i;
    zs_outf("Selected:     %U files, %U folders, %s\n", p->files, p->dirs, a);
    if (p->ratio_permille < 1000 || p->bytes) zs_outf("Archive size: about %s (%s%% of the original, sampled), at most %s\n", b, pct, c);
    zs_outf("Free space:   %s on %s%s%s%s (keeping %s free)\n", f, p->dest_dir, p->fs_name ? " [" : "", p->fs_name ? p->fs_name : "", p->fs_name ? "]" : "", m);
    if (p->delete_originals) {
        char *sv = zs_human(p->same_volume_bytes);
        zs_outf("Deletion:     originals deleted as they are archived; %s of them are on the destination drive\n", sv);
        zs_free(sv);
    }
    zs_outf("Result:       %s\n", p->explanation);
    for (i = 0; i < p->warnings.len; i++) warn((const char *)p->warnings.v[i]);
    for (i = 0; i < s->nwarnings; i++) warn(s->warnings[i]);
    if (s->nskips) {
        char *t = zs_fmt("%U items will not be archived (and will not be deleted):", (u64)s->nskips);
        warn(t);
        zs_free(t);
        for (i = 0; i < s->nskips && i < 20; i++) zs_errf("  %s: %s\n", s->skips[i].path, s->skips[i].reason);
        if (s->nskips > 20) zs_errf("  ... and %U more\n", (u64)(s->nskips - 20));
    }
    zs_free(pct), zs_free(a), zs_free(b), zs_free(c), zs_free(f), zs_free(m);
}

static void print_parts(const Part *parts, int n, u64 part_size) {
    int i, over = 0;
    if (n <= 1) {
        if (part_size) zs_out("Everything fits in one ZIP file of that size, so it is not split.\n");
        return;
    }
    zs_outf("Split into %d separate ZIP files:\n", n);
    for (i = 0; i < n; i++) {
        char *b = zs_human(parts[i].bytes), *e = zs_human(parts[i].estimated), *w = zs_human(parts[i].worst);
        zs_outf("  %s  %U files, %s of data -> about %s (at most %s)\n", zs_basename(parts[i].output), parts[i].files, b, e, w);
        zs_free(b), zs_free(e), zs_free(w);
        if (part_size && parts[i].files == 1 && parts[i].estimated > part_size) over++;
    }
    if (over) {
        char *t = zs_human(part_size);
        zs_outf("%d of them hold a single file larger than %s; a file is never cut in two.\n", over, t);
        zs_free(t);
    }
}

typedef struct {
    char *output;
    char **roots;
    size_t nroots;
    Scan scan;
    JobPaths paths;
    Preflight pf;
    Part *parts;
    int nparts;
} Checked;

static void checked_free(Checked *c) {
    size_t i;
    zs_free(c->output);
    for (i = 0; i < c->nroots; i++) zs_free(c->roots[i]);
    zs_free(c->roots);
    scan_free(&c->scan);
    paths_free(&c->paths);
    preflight_free(&c->pf);
    if (c->parts) parts_free(c->parts, c->nparts);
}

static int scan_and_check(const Args *a, const Options *o, Checked *c) {
    char *e = NULL;
    const char *excl[3];
    memset(c, 0, sizeof(*c));
    if (!a->pos.len) return err("name at least one file or folder to archive"), EX_USAGE;
    if (!a->output) return err("name the archive to create with -o NAME.zip"), EX_USAGE;
    if (!job_validate((const char *const *)a->pos.v, a->pos.len, a->output, &c->output, &c->roots, &c->nroots, &e)) return err(e), zs_free(e), EX_ERROR;
    paths_for_output(&c->paths, c->output);
    excl[0] = c->paths.output, excl[1] = c->paths.partial, excl[2] = c->paths.journal;
    c->scan.roots = NULL;
    if (!scan_build(&c->scan, c->roots, c->nroots, excl, 3, &g_cancel, &e)) return err(e), zs_free(e), g_cancel ? EX_INTERRUPTED : EX_ERROR;
    if (!preflight(&c->scan, &c->paths, o, &c->pf, &e)) return err(e), zs_free(e), EX_ERROR;
    if (a->parts > 1) {
        if (!split_scan(&c->scan, c->paths.output, a->parts, c->pf.ratio_permille, &c->parts, &e)) return err(e), zs_free(e), EX_ERROR;
        c->nparts = a->parts;
    } else if (a->part_size) {
        if (!split_scan_size(&c->scan, c->paths.output, a->part_size, c->pf.ratio_permille, &c->parts, &c->nparts, &e)) return err(e), zs_free(e), EX_ERROR;
    }
    return EX_OK;
}

static void make_options(const Args *a, Options *o) {
    options_default(o);
    o->level = a->level;
    o->delete_originals = a->del;
    o->delete_readonly = a->delro;
    o->min_free = a->min_free;
    o->quick_check = a->quick_change;
    o->overwrite = a->overwrite;
    o->final_verify = !a->no_final;
    if (a->batch) o->batch_entries = a->batch;
    o->force_zip64 = a->force_zip64;
}

static int cmd_check(const Args *a) {
    Options o;
    Checked c;
    int code;
    make_options(a, &o);
    code = scan_and_check(a, &o, &c);
    if (code == EX_OK) {
        print_preflight(&c.pf, &c.scan);
        print_parts(c.parts, c.nparts, a->part_size);
        if (paths_unfinished(&c.paths)) {
            char *t = zs_fmt("an unfinished job exists for this archive: run zipshrink resume \"%s\"", c.paths.partial);
            warn(t), zs_free(t);
        } else if (plat_exists(c.paths.output)) {
            char *t = zs_fmt("%s already exists; creating it would need --overwrite", c.paths.output);
            warn(t), zs_free(t);
        }
        if (c.pf.verdict == V_NO) code = EX_NO_SPACE;
    }
    checked_free(&c);
    return code;
}

/* ---------- running jobs ---------- */

typedef struct {
    int tty, verbose, quiet;
    u32 last;
    int drawn;
    Phase phase;
} Ui;

static const char *phase_word(Phase p) {
    switch (p) {
    case PH_SCAN: return "Scanning";
    case PH_RECOVER: return "Recovering";
    case PH_COMPRESS: return "Compressing";
    case PH_FINALIZE: return "Finalizing";
    case PH_VERIFY: return "Verifying";
    case PH_CLEANUP: return "Cleaning up";
    default: return "Done";
    }
}

static void clear_line(Ui *u) {
    if (u->drawn) zs_err("\r                                                                               \r");
    u->drawn = 0;
}

/* The last n characters (not bytes) of a UTF-8 name, with "..." in front. */
static char *shorten(const char *s, size_t n) {
    size_t chars = 0;
    const char *p;
    for (p = s; *p; p++)
        if ((*p & 0xC0) != 0x80) chars++;
    if (chars <= n) return zs_strdup(s);
    for (p = s; chars > n - 3; p++)
        if ((*p & 0xC0) != 0x80) chars--;
    while ((*p & 0xC0) == 0x80) p++;
    return zs_join("...", p);
}

static void on_phase(void *ctx, Phase p) { ((Ui *)ctx)->phase = p; }

static void on_progress(void *ctx, const Stats *s) {
    Ui *u = ctx;
    u32 now = zs_ticks(), pm;
    Buf b = {0};
    char bar[21], *name, *arch;
    int i;
    if (!u->tty || now - u->last < 250) return;
    u->last = now;
    pm = stats_permille(s);
    for (i = 0; i < 20; i++) bar[i] = (u32)i * 50 < pm ? '#' : '.';
    bar[20] = 0;
    arch = zs_human(s->archive_size);
    name = s->has_current && s->phase == PH_COMPRESS ? shorten(s->cur_name, 22) : zs_strdup("");
    buf_fmt(&b, "\r%s [%s] %3u%% %U/%U files, zip %s %s", phase_word(u->phase), bar, pm / 10, s->files_archived, s->total_files, arch, name);
    if (b.len > 80) b.len = 80;
    while (b.len < 80) buf_putc(&b, ' ');
    zs_err(buf_cstr(&b));
    u->drawn = 1;
    buf_free(&b), zs_free(arch), zs_free(name);
}

static void on_entry(void *ctx, u64 idx, const char *name, u64 size, Status st, const char *detail) {
    Ui *u = ctx;
    const char *word = NULL;
    if (st == ST_FAILED) word = "failed";
    else if (st == ST_KEPT) word = "kept";
    else if (st == ST_DELETED && u->verbose) word = "deleted";
    else if (st == ST_ARCHIVED && u->verbose) word = "archived";
    if (!word) return;
    clear_line(u);
    if (detail && *detail) zs_errf("%s %s: %s\n", word, name, detail);
    else zs_errf("%s %s\n", word, name);
}

static void on_message(void *ctx, const char *text) {
    clear_line(ctx);
    zs_errf("%s\n", text);
}

static void print_list(const char *title, const Note *v, size_t n) {
    size_t i;
    char *t;
    if (!n) return;
    t = zs_fmt("%s (%U):", title, (u64)n);
    warn(t), zs_free(t);
    for (i = 0; i < n && i < 50; i++) zs_errf("  %s: %s\n", v[i].path, v[i].reason);
    if (n > 50) zs_errf("  ... and %U more\n", (u64)(n - 50));
}

static void print_issues(const Report *r) {
    size_t i;
    print_list("not archived, left in place", r->failed, r->nfailed);
    print_list("skipped", r->skipped, r->nskipped);
    print_list("archived, original kept", r->kept, r->nkept);
    for (i = 0; i < r->warnings.len; i++) warn((const char *)r->warnings.v[i]);
}

static int print_report(const Report *r) {
    if (r->outcome != OUT_STOPPED) {
        zs_outf("\nArchive created: %s\n", r->paths.output);
        print_human("  Original size:   %s\n", r->original_bytes);
        {
            char *h = zs_human(r->archive_size), *p = percent(r->archive_size, r->original_bytes);
            if (r->original_bytes) zs_outf("  Archive size:    %s (%s%% of original)\n", h, p);
            else zs_outf("  Archive size:    %s\n", h);
            zs_free(h), zs_free(p);
        }
        zs_outf("  Archived:        %U files, %U folders\n", r->files_archived, r->dirs_archived);
        if (r->delete_requested) {
            char *h = zs_human(r->reclaimed);
            zs_outf("  Originals:       %U deleted, %U kept; %s reclaimed; %U emptied folders removed\n", r->deleted, (u64)r->nkept, h, r->dirs_removed);
            zs_free(h);
        } else {
            zs_out("  Originals:       untouched\n");
        }
        if (r->verified) {
            const ArchiveCheck *v = &r->check;
            if (v->data_checked)
                zs_outf("  Verification:    %s - %U entries; %U files decompressed and matched by SHA-256 and CRC-32\n", archive_ok(v) ? "passed" : "FAILED", v->entries,
                        v->sha_checked + v->crc_only);
            else
                zs_outf("  Verification:    %s - %U entries; structure checked (data was verified as each entry was written)\n", archive_ok(v) ? "passed" : "FAILED",
                        v->entries);
        }
        print_issues(r);
        return r->outcome == OUT_COMPLETED ? EX_OK : EX_ISSUES;
    }
    zs_out("\n");
    err(r->error ? r->error : "the job stopped");
    print_issues(r);
    if (r->resumable) {
        zs_errf("\nThe job is paused. %U entries are safely stored in %s.\nTo continue: zipshrink resume \"%s\"\n", r->recoverable, r->paths.partial, r->paths.partial);
    } else if (r->stop == STOP_DAMAGED || r->stop == STOP_VERIFY) {
        zs_errf("\nNo further originals will be deleted. Do not delete %s or %s.\nTo inspect: zipshrink status \"%s\"\n"
                "To rebuild a readable archive from what is intact: zipshrink salvage \"%s\"\n",
                r->paths.partial, r->paths.journal, r->paths.partial, r->paths.partial);
    }
    switch (r->stop) {
    case STOP_CANCELLED: return EX_INTERRUPTED;
    case STOP_LOWSPACE:
    case STOP_DISKFULL: return EX_NO_SPACE;
    case STOP_DAMAGED: return EX_DAMAGED;
    case STOP_VERIFY: return EX_VERIFY;
    default: return r->resumable ? EX_INTERRUPTED : EX_ERROR;
    }
}

static int run_job(Job *j, const Args *a) {
    Ui u;
    Observer o;
    Report *r;
    int code;
    memset(&u, 0, sizeof(u));
    u.tty = is_console(STD_ERROR_HANDLE) && !a->quiet;
    u.verbose = a->verbose, u.quiet = a->quiet;
    o.ctx = &u, o.phase = on_phase, o.entry = on_entry, o.progress = on_progress, o.message = on_message;
    r = job_run(j, &o, &g_cancel);
    clear_line(&u);
    code = print_report(r);
    report_free(r);
    return code;
}

/* Runs jobs in order, stopping at the first that does not finish. Frees them. */
static int run_jobs(Job **jobs, size_t n, const Args *a) {
    size_t i;
    int worst = EX_OK, code = EX_OK;
    for (i = 0; i < n; i++) {
        if (code != EX_OK && code != EX_ISSUES) {
            job_free(jobs[i]);
            continue;
        }
        if (n > 1 && !a->quiet) zs_outf("\n== Part %U of %U: %s\n", (u64)(i + 1), (u64)n, job_output(jobs[i]));
        code = run_job(jobs[i], a);
        job_free(jobs[i]);
        if (code != EX_OK && code != EX_ISSUES) {
            if (n > 1 && i + 1 < n) zs_errf("Parts %U to %U were not started; zipshrink resume continues them too.\n", (u64)(i + 2), (u64)n);
            worst = code;
        } else if (code > worst) {
            worst = code;
        }
    }
    return worst;
}

static int cmd_create(const Args *a) {
    Options o;
    Checked c;
    int code;
    char *e = NULL;
    ErrKind k;
    Job **jobs = NULL;
    size_t njobs = 0;
    make_options(a, &o);
    code = scan_and_check(a, &o, &c);
    if (code != EX_OK) return checked_free(&c), code;
    if (!a->quiet) {
        print_preflight(&c.pf, &c.scan);
        print_parts(c.parts, c.nparts, a->part_size);
    }
    if (c.pf.verdict == V_NO && !a->ignore_space) {
        err("not enough disk space; nothing was changed (use --ignore-space-check to try anyway; the job pauses safely if space runs out)");
        return checked_free(&c), EX_NO_SPACE;
    }
    if (a->del && !a->yes) {
        char *what = describe_sources(a->pos.len, (const char *)a->pos.v[0]);
        int ok = confirm_deletion(what);
        zs_free(what);
        if (!ok) {
            zs_err("Cancelled; nothing was changed.\n");
            return checked_free(&c), EX_ERROR;
        }
    }
    if (c.nparts > 1) {
        if (!jobs_create_parts(&o, c.roots, c.nroots, c.parts, c.nparts, &jobs, &e, &k)) {
            err(e), zs_free(e);
            return checked_free(&c), code_for(k);
        }
        njobs = (size_t)c.nparts;
    } else {
        Job *j = job_create(&o, c.roots, c.nroots, &c.scan, c.output, &e, &k);
        if (!j) {
            err(e), zs_free(e);
            return checked_free(&c), code_for(k);
        }
        jobs = zs_alloc(sizeof(Job *));
        jobs[0] = j;
        njobs = 1;
    }
    checked_free(&c);
    code = run_jobs(jobs, njobs, a);
    zs_free(jobs);
    return code;
}

static int cmd_resume(const Args *a) {
    PtrVec targets;
    Job **jobs;
    size_t i, n;
    int any_delete = 0;
    if (a->pos.len != 1) return err("name the job to resume: its .partial file, its .journal, or the archive name"), EX_USAGE;
    targets = resume_targets((const char *)a->pos.v[0]);
    n = targets.len;
    if (!n) {
        char *t = zs_fmt("no unfinished job was found for %s", (const char *)a->pos.v[0]);
        err(t), zs_free(t);
        return EX_ERROR;
    }
    jobs = zs_zalloc(n * sizeof(Job *));
    for (i = 0; i < n; i++) {
        char *e = NULL;
        ErrKind k;
        jobs[i] = job_open((const char *)targets.v[i], &e, &k);
        if (!jobs[i]) {
            size_t m;
            err(e), zs_free(e);
            for (m = 0; m < i; m++) job_free(jobs[m]);
            zs_free(jobs), pv_free(&targets, 1);
            return k == ERR_BUSY ? EX_ERROR : code_for(k);
        }
        if (a->has_min_free) job_set_min_free(jobs[i], a->min_free);
        if (a->no_delete) job_disable_delete(jobs[i]);
        if (job_options(jobs[i])->delete_originals) any_delete = 1;
    }
    pv_free(&targets, 1);
    if (any_delete && !a->yes) {
        char *what = describe_sources(job_nsources(jobs[0]), job_source(jobs[0], 0));
        int ok;
        zs_err("This job was started with --delete-originals (use --no-delete to stop deleting).\n");
        ok = confirm_deletion(what);
        zs_free(what);
        if (!ok) {
            zs_err("Cancelled; nothing was changed.\n");
            for (i = 0; i < n; i++) job_free(jobs[i]);
            zs_free(jobs);
            return EX_ERROR;
        }
    }
    if (!a->quiet)
        for (i = 0; i < n; i++) zs_outf("Resuming %s\n", job_partial(jobs[i]));
    {
        int code = run_jobs(jobs, n, a);
        zs_free(jobs);
        return code;
    }
}

static int cmd_verify(const Args *a) {
    ArchiveCheck c;
    char *e = NULL;
    const char *path;
    size_t i;
    if (a->pos.len != 1) return err("name one archive to verify"), EX_USAGE;
    path = (const char *)a->pos.v[0];
    if (!check_archive(path, !a->quick, &g_cancel, NULL, NULL, &c, &e)) {
        int code = g_cancel ? EX_INTERRUPTED : EX_VERIFY;
        err(e), zs_free(e);
        archive_check_free(&c);
        return code;
    }
    if (archive_ok(&c)) {
        char *h = zs_human(c.bytes);
        zs_outf("Verified: %s\n  %U entries (%U files, %U folders), %s\n", path, c.entries, c.files, c.dirs, h);
        zs_free(h);
        if (c.data_checked) {
            zs_outf("  %U files matched their stored SHA-256 and CRC-32\n", c.sha_checked);
            if (c.crc_only) zs_outf("  %U files have no stored SHA-256 (not made by ZipShrink) and matched their CRC-32\n", c.crc_only);
        } else {
            zs_out("  structure only (--quick); data was not decompressed\n");
        }
        archive_check_free(&c);
        return EX_OK;
    }
    zs_errf("error: %s failed verification:\n", path);
    for (i = 0; i < c.problems.len && i < 100; i++) zs_errf("  %s\n", (const char *)c.problems.v[i]);
    if (c.problems.len > 100) zs_errf("  ... and %U more\n", (u64)(c.problems.len - 100));
    archive_check_free(&c);
    return EX_VERIFY;
}

static int cmd_status(const Args *a) {
    JobPaths p;
    Loaded l;
    char *e = NULL;
    int busy;
    size_t i;
    u64 planned = 0, files = 0, bytes = 0, commits = 0, cbytes = 0, deleted = 0, kept = 0, failed = 0, end = 0;
    int finalizing = 0, finalized = 0, del = 0;
    HANDLE h;
    if (a->pos.len != 1) return err("name the job: its .partial file, its .journal, or the archive name"), EX_USAGE;
    paths_from_any(&p, (const char *)a->pos.v[0]);
    if (!journal_load(p.journal, &l, &e, &busy)) {
        int code = plat_exists(p.journal) ? EX_DAMAGED : EX_ERROR;
        zs_errf("error: %s: %s\n", p.journal, e);
        if (busy) zs_err("The job is open in another ZipShrink window or process.\n");
        zs_free(e), paths_free(&p);
        return busy ? EX_ERROR : code;
    }
    for (i = 0; i < l.count; i++) {
        const Rec *r = &l.recs[i];
        size_t k;
        switch (r->type) {
        case 'H':
            zs_outf("Job %s created %U by ZipShrink %s on %s\n", rec_get(r, "job") ? rec_get(r, "job") : "?", rec_u64(r, "created"), rec_get(r, "app") ? rec_get(r, "app") : "?",
                    rec_get(r, "os") ? rec_get(r, "os") : "?");
            zs_outf("Output:  %s\n", rec_get(r, "out") ? rec_get(r, "out") : "?");
            for (k = 0; k < (size_t)r->n; k++)
                if (!zs_strcmp(r->key[k], "src")) zs_outf("Source:  %s\n", r->val[k]);
            del = rec_u64(r, "del") != 0;
            break;
        case 'P':
            planned++;
            if (!rec_u64(r, "dir")) files++, bytes += rec_u64(r, "size");
            break;
        case 'C':
            commits++;
            cbytes += rec_u64(r, "us");
            end = rec_u64(r, "end");
            break;
        case 'X':
            if (!rec_u64(r, "missing")) deleted++;
            break;
        case 'K': kept++; break;
        case 'F': failed++; break;
        case 'S': finalizing = 1; break;
        case 'Z': finalized = 1; break;
        }
    }
    {
        char *hb = zs_human(bytes), *hc = zs_human(cbytes);
        zs_outf("Planned: %U entries (%U files, %s)\n", planned, files, hb);
        zs_outf("Safely archived: %U entries, %s of data, ending at byte %U\n", commits, hc, end);
        zs_free(hb), zs_free(hc);
    }
    if (del) zs_outf("Deletion of originals: on - %U deleted, %U kept\n", deleted, kept);
    else zs_out("Deletion of originals: off\n");
    if (failed) zs_outf("Failures recorded: %U (retried on resume)\n", failed);
    h = plat_open_read(p.partial, 0);
    if (h != INVALID_HANDLE_VALUE) {
        u64 len = plat_file_len(h);
        CloseHandle(h);
        if (len >= end) zs_outf("Partial archive: %s (%U bytes; %U beyond the last commit will be discarded)\n", p.partial, len, len - end);
        else zs_outf("Partial archive: %s is SHORTER than committed (%U < %U): damaged\n", p.partial, len, end);
    } else if (finalized) {
        zs_out("Partial archive: already renamed to the output\n");
    } else {
        zs_out("Partial archive: missing\n");
    }
    if (l.torn_tail) zs_out("The last journal record was cut off by the interruption and will be ignored.\n");
    zs_outf("State: %s\n", finalized ? "finished; only cleanup remains" : finalizing ? "interrupted while finalizing" : "interrupted while compressing");
    zs_outf("To continue: zipshrink resume \"%s\"\n", p.partial);
    journal_free_loaded(&l);
    paths_free(&p);
    return EX_OK;
}

static int cmd_salvage(const Args *a) {
    JobPaths p;
    SalvageReport r;
    char *out, *e = NULL;
    ErrKind k;
    const char *partial;
    if (a->pos.len != 1) return err("name the .partial archive to salvage"), EX_USAGE;
    partial = (const char *)a->pos.v[0];
    paths_from_any(&p, partial);
    if (plat_exists(p.journal)) {
        char *t = zs_fmt("a journal exists for this job; zipshrink resume \"%s\" is the normal way to finish it. Salvage ignores the journal.", p.partial);
        warn(t), zs_free(t);
    }
    out = a->output ? plat_full_path(a->output) : salvage_default_output(partial);
    if (!out) return err("the output name is not valid"), paths_free(&p), EX_ERROR;
    if (!salvage(partial, out, &g_cancel, &r, &e, &k)) {
        int code = g_cancel ? EX_INTERRUPTED : code_for(k);
        err(e), zs_free(e), zs_free(out), paths_free(&p);
        return code;
    }
    {
        char *h = zs_human(r.bytes);
        zs_outf("Recovered: %s\n  %U entries (%U files, %U folders), %s\n", r.output, r.entries, r.files, r.dirs, h);
        zs_out("  every recovered file matched its CRC-32 (and SHA-256 where stored)\n");
        zs_free(h);
    }
    if (r.damaged_ranges) {
        char *t = zs_fmt("%U ranges of the partial archive held no complete entry (an interrupted write, or damage) and were skipped", r.damaged_ranges);
        warn(t), zs_free(t);
    }
    if (plat_exists(p.journal)) zs_outf("The old journal %s is no longer needed by this archive; keep it until you have checked the result.\n", p.journal);
    zs_free(r.output);
    archive_check_free(&r.check);
    zs_free(out);
    paths_free(&p);
    return EX_OK;
}

static void on_download(void *ctx, u64 done, u64 total) {
    char *d = zs_human(done), *t = zs_human(total);
    (void)ctx;
    if (is_console(STD_ERROR_HANDLE)) zs_errf("\rDownloading %s of %s      ", d, total ? t : "?");
    zs_free(d), zs_free(t);
}

static int cmd_update(const Args *a) {
    Release rel;
    int newer;
    char *e = NULL, *path = NULL;
    zs_outf("ZipShrink %s - checking %s for updates...\n", ZS_VERSION, UPDATE_REPO);
    if (!update_check(&rel, &newer, &e)) return err(e), zs_free(e), EX_ERROR;
    if (!newer) {
        zs_out("You have the latest version.\n");
        release_free(&rel);
        return EX_OK;
    }
    zs_outf("Update: ZipShrink %s is available: %s\n", rel.version, rel.page);
    if (a->check) return release_free(&rel), EX_OK;
    if (!update_can_install()) {
        zs_out("Download it from the release page (this copy is portable).\n");
        update_open_url(rel.page);
        return release_free(&rel), EX_OK;
    }
    if (!update_download(&rel, &path, on_download, NULL, &e)) {
        zs_err("\n");
        err(e), zs_free(e), release_free(&rel);
        return EX_ERROR;
    }
    zs_errf("\nVerified the SHA-256 of %s; starting the installer.\n", path);
    if (!update_launch(path, &e)) {
        err(e), zs_free(e), zs_free(path), release_free(&rel);
        return EX_ERROR;
    }
    zs_free(path), release_free(&rel);
    return EX_OK;
}

static int run(void) {
    int argc, code;
    char **argv = zs_args(&argc);
    const char *cmd;
    Args a;
    if (argc < 2 || !zs_strcmp(argv[1], "-h") || !zs_strcmp(argv[1], "--help") || !zs_strcmp(argv[1], "help")) {
        zs_out(USAGE);
        return argc < 2 ? EX_USAGE : EX_OK;
    }
    if (!zs_strcmp(argv[1], "-V") || !zs_strcmp(argv[1], "--version")) {
        zs_outf("zipshrink %s (%s)\n", ZS_VERSION, zs_os_name());
        return EX_OK;
    }
    cmd = argv[1];
    if (!parse_args(argc, argv, 2, &a)) return EX_USAGE;
    SetConsoleCtrlHandler(on_ctrl, TRUE);
    if (!zs_strcmp(cmd, "create")) code = cmd_create(&a);
    else if (!zs_strcmp(cmd, "check")) code = cmd_check(&a);
    else if (!zs_strcmp(cmd, "resume")) code = cmd_resume(&a);
    else if (!zs_strcmp(cmd, "verify")) code = cmd_verify(&a);
    else if (!zs_strcmp(cmd, "status")) code = cmd_status(&a);
    else if (!zs_strcmp(cmd, "salvage")) code = cmd_salvage(&a);
    else if (!zs_strcmp(cmd, "update")) code = cmd_update(&a);
    else {
        zs_errf("error: unknown command %s (see zipshrink --help)\n", cmd);
        code = EX_USAGE;
    }
    pv_free(&a.pos, 0);
    return code;
}

void zs_cli_start(void) { ExitProcess((UINT)run()); }
