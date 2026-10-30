#include <stdarg.h>
#include "engine.h"
#include "journal.h"
#include "verify.h"
#include "failpoint.h"

#define BATCH_BYTES (64ull << 20)
#define BATCH_ENTRIES 1024
#define BATCH_MS 3000
#define SPACE_CHECK_EVERY (16ull << 20)
#define IO_BUF (1u << 20)
#define CHANGE_RETRIES 3

void options_default(Options *o) {
    memset(o, 0, sizeof(*o));
    o->level = 6;
    o->min_free = DEFAULT_MIN_FREE;
    o->final_verify = 1;
}

const char *phase_label(Phase p) {
    switch (p) {
    case PH_SCAN: return "Scanning";
    case PH_RECOVER: return "Checking the interrupted job";
    case PH_COMPRESS: return "Compressing";
    case PH_FINALIZE: return "Writing the central directory";
    case PH_VERIFY: return "Verifying the finished archive";
    case PH_CLEANUP: return "Removing emptied folders";
    default: return "Done";
    }
}

const char *status_label(Status s) {
    switch (s) {
    case ST_WAITING: return "Waiting";
    case ST_COMPRESSING: return "Compressing";
    case ST_VERIFYING: return "Verifying";
    case ST_ARCHIVED: return "Safely archived";
    case ST_DELETED: return "Original deleted";
    case ST_KEPT: return "Archived, original kept";
    case ST_SKIPPED: return "Skipped";
    default: return "Failed";
    }
}

u32 stats_permille(const Stats *s) {
    if (s->phase == PH_DONE) return 1000;
    if (s->phase == PH_VERIFY && s->verify_total) return (u32)(s->verify_done * 1000 / s->verify_total);
    if (!s->total_bytes) {
        u64 t = s->total_files + s->total_dirs;
        return t ? (u32)((s->files_archived + s->dirs_archived) * 1000 / t) : 0;
    }
    {
        u64 p = s->bytes_processed > s->total_bytes ? s->total_bytes : s->bytes_processed;
        /* Divide first for huge totals so p * 1000 cannot overflow. */
        u64 r = s->total_bytes > 0xFFFFFFFFFFFFULL ? p / (s->total_bytes / 1000) : p * 1000 / s->total_bytes;
        return (u32)(r > 1000 ? 1000 : r);
    }
}

u64 stats_remaining_files(const Stats *s) {
    u64 done = s->files_archived + s->failed;
    return s->total_files > done ? s->total_files - done : 0;
}

typedef struct {
    u64 idx, offset, end;
    u16 method;
    u32 crc;
    u64 csize, usize;
    u8 sha[32];
    int has_sha, zip64_local;
    u32 header_crc;
    int has_src;
    FStat src;
    u64 mtime, atime, btime;
    u32 attrs;
} Commit;

enum { DEL_NONE = 0, DEL_GONE, DEL_WASMISSING, DEL_WASKEPT };

struct Job {
    JobPaths paths;
    Options opt;
    char **sources;
    size_t nsources;
    char *job_id;
    Scan plan;
    Commit *commits;
    size_t ncommits, capc;
    long *at; /* plan idx -> commit index, -1 */
    char **failed;
    u8 *del;
    char **del_why;
    Journal jr;
    HANDLE arch;
    u64 arch_len;
    int finalized, resumed;
    Stats st;
    u32 started, last_tick, speed_t;
    u64 speed_b;
    int all_uncached, any_readback;
    ArchiveCheck check;
    int verified;
    u64 dirs_removed;
    PtrVec warnings;
    Observer *obs;
    volatile LONG *cancel;
    /* stop state */
    StopKind stop;
    int resumable;
    char *error;
};

/* ---------- helpers ---------- */

static void set_stop(Job *j, StopKind k, int resumable, const char *fmt, ...) {
    Buf b = {0};
    va_list ap;
    if (j->stop != STOP_NONE) return; /* the first cause wins */
    va_start(ap, fmt);
    zs_vfmt(&b, fmt, ap);
    va_end(ap);
    j->stop = k, j->resumable = resumable;
    zs_free(j->error);
    j->error = buf_cstr(&b);
}

static void stop_io(Job *j, const char *what, DWORD e) {
    if (plat_is_disk_full(e))
        set_stop(j, STOP_DISKFULL, 1, "the disk is full: %s. Everything committed so far is safe. Free up space, then resume.", plat_error(e));
    else set_stop(j, STOP_OTHER, 1, "%s: %s", what, plat_error(e));
}

static void tick(Job *j, int force) {
    u32 now = zs_ticks();
    if (!force && now - j->last_tick < 100) return;
    j->last_tick = now;
    if (now - j->speed_t >= 500) {
        u64 inst = (j->st.bytes_processed > j->speed_b ? j->st.bytes_processed - j->speed_b : 0) * 1000 / (now - j->speed_t);
        j->st.speed = j->st.speed ? (j->st.speed * 6 + inst * 4) / 10 : inst;
        j->speed_t = now, j->speed_b = j->st.bytes_processed;
    }
    j->st.elapsed_ms = now - j->started;
    j->st.archive_size = j->arch_len;
    {
        char *dir = zs_dirname(j->paths.output);
        u64 f;
        if (plat_free_space(dir, &f)) j->st.free_space = f;
        zs_free(dir);
    }
    if (j->obs && j->obs->progress) j->obs->progress(j->obs->ctx, &j->st);
}

static void set_phase(Job *j, Phase p) {
    j->st.phase = p;
    if (j->obs && j->obs->phase) j->obs->phase(j->obs->ctx, p);
    tick(j, 1);
}

static void status(Job *j, u64 idx, Status s, const char *detail) {
    const PlanEntry *e = &j->plan.e[idx];
    if (j->st.has_current && j->st.cur_size == e->size && !zs_strcmp(j->st.cur_name, e->name)) j->st.cur_status = s;
    if (j->obs && j->obs->entry) j->obs->entry(j->obs->ctx, idx, e->name, e->size, s, detail);
}

static void message(Job *j, const char *m) {
    if (j->obs && j->obs->message) j->obs->message(j->obs->ctx, m);
}

static int arch_append(Job *j, const void *data, u32 n) {
    if (zs_fail("archive_write")) return 0;
    if (!plat_pwrite(j->arch, data, n, j->arch_len)) return 0;
    j->arch_len += n;
    return 1;
}

static int jsync(Job *j) {
    if (!journal_sync(&j->jr)) {
        stop_io(j, j->paths.journal, GetLastError());
        return 0;
    }
    return 1;
}

static void commit_record(Buf *b, const Commit *c) {
    char sha[65];
    rec_begin(b, 'C');
    rec_num(b, "i", c->idx);
    rec_num(b, "off", c->offset);
    rec_num(b, "end", c->end);
    rec_num(b, "m", c->method);
    rec_num(b, "crc", c->crc);
    rec_num(b, "cs", c->csize);
    rec_num(b, "us", c->usize);
    if (c->has_sha) hex_encode(c->sha, 32, sha), rec_str(b, "sha", sha);
    rec_num(b, "z", (u64)c->zip64_local);
    rec_num(b, "hc", c->header_crc);
    if (c->has_src) {
        rec_num(b, "ss", c->src.size);
        rec_num(b, "smt", c->src.mtime);
        rec_num(b, "svol", c->src.volume);
        rec_num(b, "sidx", c->src.index);
        rec_num(b, "sattr", c->src.attrs);
    }
    rec_num(b, "mt", c->mtime);
    rec_num(b, "at", c->atime);
    rec_num(b, "bt", c->btime);
    rec_num(b, "attr", c->attrs);
}

static void commit_from_rec(Commit *c, const Rec *r) {
    const char *sha = rec_get(r, "sha");
    memset(c, 0, sizeof(*c));
    c->idx = rec_u64(r, "i"), c->offset = rec_u64(r, "off"), c->end = rec_u64(r, "end");
    c->method = (u16)rec_u64(r, "m"), c->crc = (u32)rec_u64(r, "crc");
    c->csize = rec_u64(r, "cs"), c->usize = rec_u64(r, "us");
    if (sha && hex_decode(sha, c->sha, 32)) c->has_sha = 1;
    c->zip64_local = (int)rec_u64(r, "z");
    c->header_crc = (u32)rec_u64(r, "hc");
    if (rec_has(r, "ss")) {
        c->has_src = 1;
        c->src.size = rec_u64(r, "ss"), c->src.mtime = rec_u64(r, "smt");
        c->src.volume = (u32)rec_u64(r, "svol"), c->src.index = rec_u64(r, "sidx"), c->src.attrs = (u32)rec_u64(r, "sattr");
    }
    c->mtime = rec_u64(r, "mt"), c->atime = rec_u64(r, "at"), c->btime = rec_u64(r, "bt"), c->attrs = (u32)rec_u64(r, "attr");
}

static void plan_record(Buf *b, const PlanEntry *e) {
    rec_begin(b, 'P');
    rec_num(b, "i", e->idx);
    rec_num(b, "root", e->root);
    rec_num(b, "size", e->size);
    rec_str(b, "name", e->name);
    rec_str(b, "src", e->src);
    rec_num(b, "dir", (u64)e->is_dir);
    rec_num(b, "mt", e->mtime);
    rec_num(b, "at", e->atime);
    rec_num(b, "bt", e->btime);
    rec_num(b, "attr", e->attrs);
}

static void write_plan(Job *j, const Scan *s) {
    Buf b = {0};
    size_t i;
    for (i = 0; i < s->nskips; i++) {
        rec_begin(&b, 's');
        rec_str(&b, "path", s->skips[i].path);
        rec_str(&b, "why", s->skips[i].reason);
        journal_add(&j->jr, &b);
    }
    for (i = 0; i < s->n; i++) plan_record(&b, &s->e[i]), journal_add(&j->jr, &b);
    buf_free(&b);
}

static void meta_of(const Job *j, const Commit *c, EntryMeta *m) {
    const PlanEntry *e = &j->plan.e[c->idx];
    m->name = e->name, m->is_dir = e->is_dir;
    m->mtime = c->mtime, m->atime = c->atime, m->btime = c->btime, m->attrs = c->attrs;
}

static void sizes_of(const Commit *c, EntrySizes *s) {
    s->method = c->method, s->crc = c->crc, s->csize = c->csize, s->usize = c->usize;
    s->has_sha = c->has_sha, s->zip64_local = c->zip64_local;
    memcpy(s->sha, c->sha, 32);
}

static void alloc_state(Job *j) {
    size_t i, n = j->plan.n ? j->plan.n : 1;
    j->at = zs_alloc(n * sizeof(long));
    for (i = 0; i < n; i++) j->at[i] = -1;
    j->failed = zs_zalloc(n * sizeof(char *));
    j->del = zs_zalloc(n);
    j->del_why = zs_zalloc(n * sizeof(char *));
}

static void push_commit(Job *j, const Commit *c) {
    if (j->ncommits == j->capc) j->capc = j->capc ? j->capc * 2 : 256, j->commits = zs_realloc(j->commits, j->capc * sizeof(Commit));
    j->commits[j->ncommits] = *c;
    j->at[c->idx] = (long)j->ncommits;
    j->ncommits++;
}

static void init_stats(Job *j) {
    size_t i;
    Stats *s = &j->st;
    memset(s, 0, sizeof(*s));
    for (i = 0; i < j->plan.n; i++) {
        if (j->plan.e[i].is_dir) s->total_dirs++;
        else s->total_files++, s->total_bytes += j->plan.e[i].size;
    }
    s->skipped = j->plan.nskips;
    s->delete_originals = j->opt.delete_originals;
    for (i = 0; i < j->ncommits; i++) {
        const Commit *c = &j->commits[i];
        if (j->plan.e[c->idx].is_dir) s->dirs_archived++;
        else {
            s->files_archived++, s->bytes_committed += c->usize, s->compressed_committed += c->csize;
            s->bytes_processed += j->plan.e[c->idx].size;
            if (j->del[c->idx] == DEL_GONE) s->deleted++, s->reclaimed += c->usize;
            if (j->del[c->idx] == DEL_WASKEPT) s->kept++;
        }
    }
    s->archive_size = j->arch_len;
}

static Job *job_alloc(void) {
    Job *j = zs_zalloc(sizeof(Job));
    j->arch = INVALID_HANDLE_VALUE;
    j->jr.h = INVALID_HANDLE_VALUE;
    j->all_uncached = 1;
    return j;
}

/* ---------- creating and opening ---------- */

int job_validate(const char *const *sources, size_t n, const char *output, char **out_abs, char ***roots, size_t *nroots, char **err) {
    char *full = plat_full_path(output), *dir;
    Kind k;
    FStat st;
    size_t i;
    if (!full || !*zs_basename(full)) {
        zs_free(full);
        return *err = zs_fmt("%s is not a valid archive file name", output), 0;
    }
    dir = zs_dirname(full);
    if (!plat_kind(dir, &k, &st) || k != K_DIR) {
        *err = zs_fmt("%s is not a folder", dir);
        zs_free(dir), zs_free(full);
        return 0;
    }
    zs_free(dir);
    if (!scan_roots(sources, n, roots, nroots, err)) return zs_free(full), 0;
    for (i = 0; i < *nroots; i++)
        if (path_within(full, (*roots)[i])) {
            *err = zs_fmt("the archive cannot be saved inside %s, which is being compressed; choose another destination", (*roots)[i]);
            size_t r;
            for (r = 0; r < *nroots; r++) zs_free((*roots)[r]);
            zs_free(*roots);
            zs_free(full);
            return 0;
        }
    *out_abs = full;
    return 1;
}

static void header_record(Buf *b, const Job *j) {
    size_t i;
    rec_begin(b, 'H');
    rec_str(b, "format", "zipshrink-journal-c1");
    rec_str(b, "job", j->job_id);
    rec_num(b, "created", (u64)zs_unix_now());
    rec_str(b, "out", j->paths.output);
    for (i = 0; i < j->nsources; i++) rec_str(b, "src", j->sources[i]);
    rec_num(b, "level", (u64)j->opt.level);
    rec_num(b, "del", (u64)j->opt.delete_originals);
    rec_num(b, "delro", (u64)j->opt.delete_readonly);
    rec_num(b, "minfree", j->opt.min_free);
    rec_num(b, "quick", (u64)j->opt.quick_check);
    rec_num(b, "overwrite", (u64)j->opt.overwrite);
    rec_num(b, "fverify", (u64)j->opt.final_verify);
    rec_num(b, "batch", j->opt.batch_entries);
    rec_num(b, "z64", (u64)j->opt.force_zip64);
    rec_str(b, "app", ZS_VERSION);
    rec_str(b, "os", zs_os_name());
}

Job *job_create(const Options *o, char **roots, size_t nroots, Scan *scan, const char *output, char **err, ErrKind *kind) {
    Job *j;
    Kind k;
    FStat st;
    char *tmp;
    Buf b = {0};
    size_t i;
    *kind = ERR_INVALID;
    if (o->level < 0 || o->level > 9) return *err = zs_strdup("compression level must be 0 to 9"), NULL;
    if (!scan->n) return *err = zs_strdup("there is nothing to archive in the selection"), NULL;
    j = job_alloc();
    paths_for_output(&j->paths, output);
    if (paths_unfinished(&j->paths)) {
        *kind = ERR_JOB_EXISTS;
        *err = zs_fmt("an unfinished job for this archive already exists (%s). Resume it, or move it away first.", j->paths.partial);
        goto fail;
    }
    if (plat_kind(j->paths.output, &k, &st) && k != K_MISSING) {
        if (k == K_DIR) {
            *err = zs_fmt("%s is a folder", j->paths.output);
            goto fail;
        }
        if (!o->overwrite) {
            *kind = ERR_EXISTS;
            *err = zs_fmt("%s already exists. Choose another name, or allow overwriting it.", j->paths.output);
            goto fail;
        }
    }
    j->opt = *o;
    if (j->opt.min_free < MIN_FREE_FLOOR) j->opt.min_free = MIN_FREE_FLOOR;
    j->sources = zs_alloc(nroots * sizeof(char *));
    for (i = 0; i < nroots; i++) j->sources[i] = zs_strdup(roots[i]);
    j->nsources = nroots;
    j->job_id = zs_fmt("%x-%x", (unsigned)GetTickCount(), (unsigned)GetCurrentProcessId());
    j->plan = *scan;
    memset(scan, 0, sizeof(*scan));
    alloc_state(j);
    /* The header goes to a temporary file that is renamed into place, so a
     * journal either does not exist or starts with a whole header. */
    tmp = zs_join(j->paths.journal, ".new");
    if (plat_exists(tmp)) {
        Loaded l;
        char *e2 = NULL;
        int busy, only_header = 0;
        if (journal_load(tmp, &l, &e2, &busy)) {
            only_header = l.count <= 1;
            journal_free_loaded(&l);
        } else {
            only_header = !busy; /* torn: it never held more than a header */
        }
        zs_free(e2);
        if (!only_header || !plat_delete_file(tmp)) {
            *kind = ERR_JOB_EXISTS;
            *err = zs_fmt("%s is in the way; another ZipShrink may be starting this job", tmp);
            zs_free(tmp);
            goto fail;
        }
    }
    if (!journal_create(&j->jr, tmp)) {
        *kind = ERR_IO;
        *err = zs_fmt("%s: %s", tmp, plat_error(GetLastError()));
        zs_free(tmp);
        goto fail;
    }
    header_record(&b, j);
    journal_add(&j->jr, &b);
    if (!journal_sync(&j->jr)) {
        *kind = ERR_IO;
        *err = zs_fmt("%s: %s", tmp, plat_error(GetLastError()));
        zs_free(tmp);
        goto fail;
    }
    {
        u64 hlen = j->jr.len;
        int busy;
        journal_close(&j->jr);
        if (!plat_rename_noreplace(tmp, j->paths.journal) || !journal_reopen(&j->jr, j->paths.journal, hlen, &busy)) {
            *kind = ERR_IO;
            *err = zs_fmt("%s: %s", j->paths.journal, plat_error(GetLastError()));
            zs_free(tmp);
            goto fail;
        }
    }
    zs_free(tmp);
    write_plan(j, &j->plan);
    if (zs_fail("before_plan_done")) {
        *kind = ERR_IO;
        *err = zs_fmt("%s: %s", j->paths.journal, plat_error(GetLastError()));
        goto fail;
    }
    rec_begin(&b, 'D');
    rec_num(&b, "entries", j->plan.n);
    rec_num(&b, "files", j->plan.files);
    rec_num(&b, "dirs", j->plan.dirs);
    rec_num(&b, "bytes", j->plan.bytes);
    journal_add(&j->jr, &b);
    if (!journal_sync(&j->jr)) {
        *kind = ERR_IO;
        *err = zs_fmt("%s: %s", j->paths.journal, plat_error(GetLastError()));
        goto fail;
    }
    j->arch = plat_create_new(j->paths.partial);
    if (j->arch == INVALID_HANDLE_VALUE || !plat_flush(j->arch)) {
        *kind = ERR_IO;
        *err = zs_fmt("%s: %s", j->paths.partial, plat_error(GetLastError()));
        goto fail;
    }
    buf_free(&b);
    init_stats(j);
    *kind = ERR_NONE;
    return j;
fail:
    buf_free(&b);
    job_free(j);
    return NULL;
}

static int load_header(Job *j, const Rec *h) {
    int i;
    options_default(&j->opt);
    for (i = 0; i < h->n; i++)
        if (!zs_strcmp(h->key[i], "src")) {
            j->sources = zs_realloc(j->sources, (j->nsources + 1) * sizeof(char *));
            j->sources[j->nsources++] = zs_strdup(h->val[i]);
        }
    j->job_id = zs_strdup(rec_get(h, "job") ? rec_get(h, "job") : "?");
    j->opt.level = (int)rec_u64(h, "level");
    j->opt.delete_originals = (int)rec_u64(h, "del");
    j->opt.delete_readonly = (int)rec_u64(h, "delro");
    j->opt.min_free = rec_u64(h, "minfree");
    j->opt.quick_check = (int)rec_u64(h, "quick");
    j->opt.overwrite = (int)rec_u64(h, "overwrite");
    j->opt.final_verify = (int)rec_u64(h, "fverify");
    j->opt.batch_entries = (u32)rec_u64(h, "batch");
    j->opt.force_zip64 = (int)rec_u64(h, "z64");
    return j->nsources > 0;
}

#define DAMAGED(...)                    \
    do {                                \
        *kind = ERR_DAMAGED;            \
        *err = zs_fmt(__VA_ARGS__);     \
        goto fail;                      \
    } while (0)

Job *job_open(const char *path, char **err, ErrKind *kind) {
    Job *j = job_alloc();
    Loaded l;
    size_t i;
    int plan_done = 0, finalize_started = 0, busy = 0;
    u64 committed_end = 0;
    memset(&l, 0, sizeof(l));
    *kind = ERR_INVALID;
    paths_from_any(&j->paths, path);
    if (!plat_exists(j->paths.journal)) {
        if (plat_exists(j->paths.partial))
            DAMAGED("%s has no journal next to it. Nothing more will be deleted. Run \"zipshrink salvage\" on it to rebuild a readable archive from the entries it holds.", j->paths.partial);
        *err = zs_fmt("no unfinished job found for %s", j->paths.output);
        goto fail;
    }
    {
        /* Another window or process holding the job locks its journal. */
        HANDLE h = plat_open_rw(j->paths.journal);
        if (h == INVALID_HANDLE_VALUE && plat_is_sharing(GetLastError())) goto busy;
        if (h != INVALID_HANDLE_VALUE) {
            int ok = plat_lock(h);
            if (ok) UnlockFile(h, 0, 0, 1, 0);
            CloseHandle(h);
            if (!ok) goto busy;
        }
    }
    if (!journal_load(j->paths.journal, &l, err, &busy)) {
        if (busy) goto busy;
        *kind = ERR_DAMAGED;
        goto fail;
    }
    if (!l.count || !load_header(j, &l.recs[0])) {
        *err = zs_strdup("the job was interrupted before it started; nothing was changed. Delete the .partial and .journal files and start again.");
        goto fail;
    }
    j->resumed = 1;
    j->plan.roots = zs_alloc(j->nsources * sizeof(char *));
    for (i = 0; i < j->nsources; i++) j->plan.roots[i] = zs_strdup(j->sources[i]);
    j->plan.nroots = j->nsources;
    /* First pass: the plan. */
    for (i = 1; i < l.count; i++) {
        const Rec *r = &l.recs[i];
        if (r->type == 'P') {
            PlanEntry e;
            memset(&e, 0, sizeof(e));
            if (rec_u64(r, "i") != j->plan.n || !rec_get(r, "name") || !rec_get(r, "src")) DAMAGED("the journal's plan is inconsistent");
            e.root = (u32)rec_u64(r, "root"), e.size = rec_u64(r, "size");
            e.name = (char *)rec_get(r, "name"), e.src = (char *)rec_get(r, "src");
            e.is_dir = (int)rec_u64(r, "dir");
            e.mtime = rec_u64(r, "mt"), e.atime = rec_u64(r, "at"), e.btime = rec_u64(r, "bt"), e.attrs = (u32)rec_u64(r, "attr");
            if (e.root >= j->nsources) DAMAGED("the journal's plan is inconsistent");
            scan_add(&j->plan, &e);
            if (e.is_dir) j->plan.dirs++;
            else j->plan.files++, j->plan.bytes += e.size;
        } else if (r->type == 's') {
            scan_note(&j->plan.skips, &j->plan.nskips, rec_get(r, "path") ? rec_get(r, "path") : "?", rec_get(r, "why") ? rec_get(r, "why") : "?");
        } else if (r->type == 'D') {
            plan_done = 1;
        }
    }
    alloc_state(j);
    /* Second pass: what happened. */
    for (i = 1; i < l.count; i++) {
        const Rec *r = &l.recs[i];
        u64 idx = rec_u64(r, "i");
        switch (r->type) {
        case 'C': {
            Commit c;
            commit_from_rec(&c, r);
            if (c.idx >= j->plan.n || j->at[c.idx] >= 0) DAMAGED("the journal's commits are inconsistent");
            if (c.offset != committed_end) DAMAGED("the journal's commits are not contiguous");
            push_commit(j, &c);
            committed_end = c.end;
            zs_free(j->failed[c.idx]);
            j->failed[c.idx] = NULL;
            break;
        }
        case 'X':
            if (idx < j->plan.n) j->del[idx] = rec_u64(r, "missing") ? DEL_WASMISSING : DEL_GONE;
            break;
        case 'K':
            if (idx < j->plan.n) {
                j->del[idx] = DEL_WASKEPT;
                zs_free(j->del_why[idx]);
                j->del_why[idx] = zs_strdup(rec_get(r, "why") ? rec_get(r, "why") : "kept");
            }
            break;
        case 'F':
            if (idx < j->plan.n && j->at[idx] < 0) {
                zs_free(j->failed[idx]);
                j->failed[idx] = zs_strdup(rec_get(r, "why") ? rec_get(r, "why") : "failed");
            }
            break;
        case 'S': finalize_started = 1; break;
        case 'Z': j->finalized = 1; break;
        case 'H': DAMAGED("the journal has two headers");
        default: break;
        }
    }
    if (!journal_reopen(&j->jr, j->paths.journal, plan_done ? l.valid_len : l.first_len, &busy)) {
        if (busy) goto busy;
        *kind = ERR_IO;
        *err = zs_fmt("%s: %s", j->paths.journal, plat_error(GetLastError()));
        goto fail;
    }
    if (!plan_done) {
        /* Interrupted while planning: nothing was written to the archive. */
        char *ex[3];
        Scan s;
        Buf b = {0};
        scan_free(&j->plan);
        ex[0] = j->paths.output, ex[1] = j->paths.partial, ex[2] = j->paths.journal;
        if (!scan_build(&s, j->sources, j->nsources, (const char *const *)ex, 3, NULL, err)) goto fail;
        j->plan = s;
        zs_free(j->at), zs_free(j->failed), zs_free(j->del), zs_free(j->del_why);
        alloc_state(j);
        write_plan(j, &j->plan);
        rec_begin(&b, 'D');
        rec_num(&b, "entries", j->plan.n);
        journal_add(&j->jr, &b);
        buf_free(&b);
        if (!journal_sync(&j->jr)) {
            *kind = ERR_IO;
            *err = zs_fmt("%s: %s", j->paths.journal, plat_error(GetLastError()));
            goto fail;
        }
    }
    if (j->finalized) {
        /* Only cleanup remained. */
    } else if (plat_exists(j->paths.partial)) {
        u64 len;
        j->arch = plat_open_rw(j->paths.partial);
        if (j->arch == INVALID_HANDLE_VALUE) {
            *kind = ERR_IO;
            *err = zs_fmt("%s: %s", j->paths.partial, plat_error(GetLastError()));
            goto fail;
        }
        len = plat_file_len(j->arch);
        if (len < committed_end)
            DAMAGED("%s is shorter (%U bytes) than the %U bytes it held when last committed. Keep the file and run \"zipshrink salvage\" to recover the entries it still contains.", j->paths.partial, len, committed_end);
        for (i = 0; i < j->ncommits; i++) {
            const Commit *c = &j->commits[i];
            u32 hlen = (u32)(c->end - c->offset - c->csize), got = 0;
            u8 *hdr = zs_alloc(hlen ? hlen : 1);
            int ok = plat_pread(j->arch, hdr, hlen, c->offset, &got) && got == hlen && crc32_update(0, hdr, hlen) == c->header_crc;
            zs_free(hdr);
            if (!ok) DAMAGED("the entry for \"%s\" at offset %U no longer matches the journal. Keep both files and run \"zipshrink salvage\".", j->plan.e[c->idx].name, c->offset);
        }
        if (len != committed_end && (!plat_set_len(j->arch, committed_end) || !plat_flush(j->arch))) {
            *kind = ERR_IO;
            *err = zs_fmt("%s: %s", j->paths.partial, plat_error(GetLastError()));
            goto fail;
        }
        j->arch_len = committed_end;
    } else if (finalize_started && plat_exists(j->paths.output)) {
        /* Crashed right after the final rename: confirm it and record it. */
        ArchiveCheck chk;
        char *e2 = NULL;
        Buf b = {0};
        if (!check_archive(j->paths.output, 1, NULL, NULL, NULL, &chk, &e2) || !archive_ok(&chk)) {
            if (e2) *err = e2;
            else *err = zs_fmt("%s failed verification", j->paths.output);
            archive_check_free(&chk);
            *kind = ERR_DAMAGED;
            goto fail;
        }
        j->check = chk, j->verified = 1;
        rec_begin(&b, 'Z');
        rec_str(&b, "path", j->paths.output);
        journal_add(&j->jr, &b);
        buf_free(&b);
        if (!journal_sync(&j->jr)) {
            *kind = ERR_IO;
            *err = zs_fmt("%s: %s", j->paths.journal, plat_error(GetLastError()));
            goto fail;
        }
        j->finalized = 1;
    } else if (j->ncommits == 0) {
        j->arch = plat_create_new(j->paths.partial);
        if (j->arch == INVALID_HANDLE_VALUE) {
            *kind = ERR_IO;
            *err = zs_fmt("%s: %s", j->paths.partial, plat_error(GetLastError()));
            goto fail;
        }
    } else {
        DAMAGED("%s is missing although the journal records %U archived entries. If you moved it, put it back next to the journal and resume again.", j->paths.partial, (u64)j->ncommits);
    }
    journal_free_loaded(&l);
    init_stats(j);
    *kind = ERR_NONE;
    return j;
busy:
    *kind = ERR_BUSY;
    zs_free(*err);
    *err = zs_strdup("this job is being processed by another ZipShrink window or process");
fail:
    journal_free_loaded(&l);
    job_free(j);
    return NULL;
}

void job_set_min_free(Job *j, u64 b) { j->opt.min_free = b < MIN_FREE_FLOOR ? MIN_FREE_FLOOR : b; }
void job_disable_delete(Job *j) {
    j->opt.delete_originals = 0;
    j->st.delete_originals = 0;
}
const Options *job_options(const Job *j) { return &j->opt; }
const char *job_output(const Job *j) { return j->paths.output; }
const char *job_partial(const Job *j) { return j->paths.partial; }
size_t job_nsources(const Job *j) { return j->nsources; }
const char *job_source(const Job *j, size_t i) { return j->sources[i]; }

void job_free(Job *j) {
    size_t i, n;
    if (!j) return;
    n = j->plan.n;
    if (j->arch != INVALID_HANDLE_VALUE) CloseHandle(j->arch);
    journal_close(&j->jr);
    for (i = 0; i < n; i++) {
        if (j->failed) zs_free(j->failed[i]);
        if (j->del_why) zs_free(j->del_why[i]);
    }
    zs_free(j->failed), zs_free(j->del_why), zs_free(j->del), zs_free(j->at);
    zs_free(j->commits);
    scan_free(&j->plan);
    for (i = 0; i < j->nsources; i++) zs_free(j->sources[i]);
    zs_free(j->sources);
    zs_free(j->job_id);
    zs_free(j->error);
    archive_check_free(&j->check);
    pv_free(&j->warnings, 1);
    paths_free(&j->paths);
    zs_free(j);
}

/* ---------- running ---------- */

static int ensure_space(Job *j, u64 need) {
    char *dir = zs_dirname(j->paths.output);
    u64 avail = 0;
    int ok = plat_free_space(dir, &avail);
    zs_free(dir);
    if (!ok) return 1; /* unknown: the write itself will report a full disk */
    if (avail < j->opt.min_free + need) {
        char *a = zs_human(avail), *m = zs_human(j->opt.min_free);
        set_stop(j, STOP_LOWSPACE, 1, "paused because free space on the destination fell to the reserve: %s free, %s reserved. Free up space, or lower the reserve, then resume.", a, m);
        zs_free(a), zs_free(m);
        return 0;
    }
    return 1;
}

typedef enum { W_OK, W_SOURCE, W_CHANGED, W_STOP } WResult;

static WResult write_dir(Job *j, const PlanEntry *e, Commit *c) {
    Buf h = {0};
    EntryMeta m;
    EntrySizes s;
    memset(&s, 0, sizeof(s));
    memset(c, 0, sizeof(*c));
    s.method = ZIP_STORE, s.zip64_local = j->opt.force_zip64;
    m.name = e->name, m.is_dir = 1, m.mtime = e->mtime, m.atime = e->atime, m.btime = e->btime, m.attrs = e->attrs;
    zip_local_header(&h, &m, &s);
    c->idx = e->idx, c->offset = j->arch_len, c->method = ZIP_STORE, c->zip64_local = s.zip64_local;
    c->header_crc = crc32_update(0, h.p, h.len);
    c->mtime = e->mtime, c->atime = e->atime, c->btime = e->btime, c->attrs = e->attrs;
    if (!arch_append(j, h.p, (u32)h.len)) {
        buf_free(&h);
        stop_io(j, j->paths.partial, GetLastError());
        return W_STOP;
    }
    c->end = j->arch_len;
    buf_free(&h);
    return W_OK;
}

typedef struct {
    Job *j;
    tdefl_compressor *z;
    u8 *out;
    size_t out_len;
    u64 since_check;
    int failed;
} Sink2;

static int sink_flush(Sink2 *k) {
    if (!k->out_len) return 1;
    if (k->since_check >= SPACE_CHECK_EVERY) {
        k->since_check = 0;
        if (!ensure_space(k->j, SPACE_CHECK_EVERY)) return k->failed = 1, 0;
    }
    if (!arch_append(k->j, k->out, (u32)k->out_len)) {
        stop_io(k->j, k->j->paths.partial, GetLastError());
        return k->failed = 1, 0;
    }
    k->since_check += k->out_len;
    k->out_len = 0;
    return 1;
}

static int sink_push(Sink2 *k, const u8 *data, size_t n, int finish) {
    if (!k->z) {
        while (n) {
            size_t take = IO_BUF - k->out_len < n ? IO_BUF - k->out_len : n;
            memcpy(k->out + k->out_len, data, take);
            k->out_len += take, data += take, n -= take;
            if (k->out_len == IO_BUF && !sink_flush(k)) return 0;
        }
        return 1;
    }
    for (;;) {
        size_t in_n = n, out_n = IO_BUF - k->out_len;
        tdefl_status st = tdefl_compress(k->z, data, &in_n, k->out + k->out_len, &out_n, finish ? TDEFL_FINISH : TDEFL_NO_FLUSH);
        data += in_n, n -= in_n, k->out_len += out_n;
        if (st < 0) {
            set_stop(k->j, STOP_OTHER, 1, "compression failed");
            return k->failed = 1, 0;
        }
        if (k->out_len == IO_BUF && !sink_flush(k)) return 0;
        if (finish ? st == TDEFL_STATUS_DONE : n == 0) break;
    }
    return 1;
}

static char *source_error(DWORD e) {
    if (plat_is_sharing(e)) return zs_strdup("could not be opened because another program is using it");
    if (e == ERROR_FILE_NOT_FOUND || e == ERROR_PATH_NOT_FOUND) return zs_strdup("no longer exists");
    if (e == ERROR_ACCESS_DENIED) return zs_fmt("could not be opened (access denied): %s", plat_error(e));
    return zs_fmt("could not be opened: %s", plat_error(e));
}

static WResult write_file(Job *j, const PlanEntry *e, Commit *c, char **why) {
    HANDLE src;
    FStat st0, st1;
    EntryMeta m;
    EntrySizes s;
    Buf h = {0}, h2 = {0};
    Sink2 k;
    u8 *buf;
    u64 total = 0, data_start, processed_before = j->st.bytes_processed;
    u32 crc = 0, last = zs_ticks();
    Sha256 sha;
    WResult res = W_OK;
    *why = NULL;
    src = plat_open_read(e->src, 1);
    if (src != INVALID_HANDLE_VALUE && zs_fail("open_source")) CloseHandle(src), src = INVALID_HANDLE_VALUE;
    if (src == INVALID_HANDLE_VALUE) return *why = source_error(GetLastError()), W_SOURCE;
    if (!plat_stat_handle(src, &st0)) {
        *why = zs_fmt("could not be examined: %s", plat_error(GetLastError()));
        CloseHandle(src);
        return W_SOURCE;
    }
    memset(&s, 0, sizeof(s));
    memset(c, 0, sizeof(*c));
    m.name = e->name, m.is_dir = 0, m.mtime = st0.mtime, m.atime = st0.atime, m.btime = st0.btime, m.attrs = st0.attrs & 0x27;
    s.method = j->opt.level ? ZIP_DEFLATE : ZIP_STORE;
    s.zip64_local = j->opt.force_zip64 || zip_needs_zip64(st0.size);
    s.has_sha = 1;
    c->idx = e->idx, c->offset = j->arch_len;
    zip_local_header(&h, &m, &s);
    if (!arch_append(j, h.p, (u32)h.len)) {
        stop_io(j, j->paths.partial, GetLastError());
        res = W_STOP;
        goto out;
    }
    if (zs_fail("entry_header_written")) {
        stop_io(j, j->paths.partial, GetLastError());
        res = W_STOP;
        goto out;
    }
    data_start = j->arch_len;
    memset(&k, 0, sizeof(k));
    k.j = j;
    k.out = zs_alloc(IO_BUF);
    if (s.method == ZIP_DEFLATE) {
        k.z = zs_alloc(sizeof(tdefl_compressor));
        tdefl_init(k.z, NULL, NULL, (int)tdefl_create_comp_flags_from_zip_params(j->opt.level, -15, 0));
    }
    buf = zs_alloc(IO_BUF);
    sha256_init(&sha);
    j->st.has_current = 1;
    for (;;) {
        u32 got = 0;
        if (j->cancel && *j->cancel) {
            set_stop(j, STOP_CANCELLED, 1, "stopped at your request");
            res = W_STOP;
            break;
        }
        if (!plat_pread(src, buf, IO_BUF, total, &got)) {
            *why = zs_fmt("could not be read: %s", plat_error(GetLastError()));
            res = W_SOURCE;
            break;
        }
        if (!got) break;
        crc = crc32_update(crc, buf, got);
        sha256_update(&sha, buf, got);
        total += got;
        if (total > st0.size) {
            *why = zs_strdup("it grew while being read");
            res = W_CHANGED;
            break;
        }
        if (!sink_push(&k, buf, got, 0)) {
            res = W_STOP;
            break;
        }
        if (zs_ticks() - last >= 100) {
            last = zs_ticks();
            j->st.cur_done = total;
            j->st.bytes_processed = processed_before + total;
            tick(j, 1);
        }
    }
    if (res == W_OK && !sink_push(&k, NULL, 0, 1)) res = W_STOP;
    if (res == W_OK && !sink_flush(&k)) res = W_STOP;
    zs_free(buf);
    zs_free(k.out);
    zs_free(k.z);
    j->st.bytes_processed = processed_before;
    if (res != W_OK) goto out;
    if (!plat_stat_handle(src, &st1)) {
        *why = zs_fmt("could not be examined: %s", plat_error(GetLastError()));
        res = W_SOURCE;
        goto out;
    }
    if (total != st0.size || !fstat_same(&st0, &st1)) {
        *why = zs_fmt("%s while being read", fstat_difference(&st0, &st1));
        res = W_CHANGED;
        goto out;
    }
    s.crc = crc, s.csize = j->arch_len - data_start, s.usize = total;
    sha256_final(&sha, s.sha);
    if (!s.zip64_local && (s.csize >= 0xFFFFFFFFull || s.usize >= 0xFFFFFFFFull)) {
        *why = zs_strdup("it grew past the size planned for its header");
        res = W_CHANGED;
        goto out;
    }
    zip_local_header(&h2, &m, &s);
    if (h2.len != h.len || !plat_pwrite(j->arch, h2.p, (u32)h2.len, c->offset) || zs_fail("entry_data_written")) {
        stop_io(j, j->paths.partial, GetLastError());
        res = W_STOP;
        goto out;
    }
    c->end = j->arch_len;
    c->method = s.method, c->crc = s.crc, c->csize = s.csize, c->usize = s.usize;
    memcpy(c->sha, s.sha, 32), c->has_sha = 1, c->zip64_local = s.zip64_local;
    c->header_crc = crc32_update(0, h2.p, h2.len);
    c->has_src = 1, c->src = st1;
    c->mtime = m.mtime, c->atime = m.atime, c->btime = m.btime, c->attrs = m.attrs;
    j->st.bytes_processed = processed_before + total;
out:
    CloseHandle(src);
    buf_free(&h);
    buf_free(&h2);
    return res;
}

static void mark_failed(Job *j, u64 idx, char *why) {
    Buf b = {0};
    status(j, idx, ST_FAILED, why);
    rec_begin(&b, 'F');
    rec_num(&b, "i", idx);
    rec_str(&b, "why", why);
    journal_add(&j->jr, &b);
    buf_free(&b);
    zs_free(j->failed[idx]);
    j->failed[idx] = why;
    j->st.failed++;
    j->st.bytes_processed += j->plan.e[idx].size;
}

static int delete_original(Job *j, const Commit *c, int full_hash) {
    const PlanEntry *e = &j->plan.e[c->idx];
    Buf b = {0};
    DelResult r;
    char *why = NULL;
    if (!c->has_src) return 1;
    if (!path_within(e->src, j->sources[e->root])) {
        r = DEL_KEPT, why = zs_strdup("kept because it is outside the selected folders");
    } else if ((c->src.attrs & FILE_ATTRIBUTE_READONLY) && !j->opt.delete_readonly) {
        r = DEL_KEPT, why = zs_strdup("kept because it is read-only");
    } else {
        if (zs_fail("before_delete")) return stop_io(j, e->src, GetLastError()), 0;
        r = plat_delete_verified(e->src, &c->src, full_hash ? c->sha : NULL, j->opt.delete_readonly, &why);
    }
    if (zs_fail("after_delete")) return zs_free(why), stop_io(j, e->src, GetLastError()), 0;
    if (r == DEL_DELETED || r == DEL_MISSING) {
        rec_begin(&b, 'X');
        rec_num(&b, "i", c->idx);
        rec_num(&b, "missing", r == DEL_MISSING);
        j->del[c->idx] = r == DEL_DELETED ? DEL_GONE : DEL_WASMISSING;
        if (r == DEL_DELETED) j->st.deleted++, j->st.reclaimed += c->usize;
        status(j, c->idx, ST_DELETED, NULL);
    } else {
        rec_begin(&b, 'K');
        rec_num(&b, "i", c->idx);
        rec_str(&b, "why", why ? why : "kept");
        j->del[c->idx] = DEL_WASKEPT;
        zs_free(j->del_why[c->idx]);
        j->del_why[c->idx] = zs_strdup(why ? why : "kept");
        j->st.kept++;
        status(j, c->idx, ST_KEPT, why);
    }
    zs_free(why);
    journal_add(&j->jr, &b);
    buf_free(&b);
    return 1;
}

static void cut_back(Job *j, u64 to) {
    if (plat_set_len(j->arch, to)) j->arch_len = to;
    plat_flush(j->arch);
}

static Expected expected_of(const Job *j, const Commit *c) {
    Expected x;
    x.name = j->plan.e[c->idx].name, x.offset = c->offset, x.end = c->end, x.method = c->method;
    x.crc = c->crc, x.csize = c->csize, x.usize = c->usize, x.sha = c->has_sha ? c->sha : NULL;
    x.zip64_local = c->zip64_local, x.header_crc = c->header_crc;
    return x;
}

static int flush_batch(Job *j, Commit *pending, size_t *np) {
    size_t i, first = j->ncommits;
    u64 first_off;
    Buf b = {0};
    if (!*np) return 1;
    first_off = pending[0].offset;
    if (zs_fail("before_archive_sync") || !plat_flush(j->arch)) {
        DWORD e = GetLastError();
        cut_back(j, first_off);
        return stop_io(j, j->paths.partial, e), 0;
    }
    if (zs_fail("batch_synced")) return stop_io(j, j->paths.partial, GetLastError()), 0;
    for (i = 0; i < *np; i++) {
        Expected x = expected_of(j, &pending[i]);
        Readback mode;
        char *why = NULL;
        CheckResult r;
        status(j, pending[i].idx, ST_VERIFYING, NULL);
        tick(j, 0);
        r = check_entry(j->arch, j->paths.partial, &x, &mode, &why);
        if (r == CHK_OK) {
            j->any_readback = 1;
            if (mode != RB_UNCACHED) j->all_uncached = 0;
            continue;
        }
        cut_back(j, first_off);
        if (r == CHK_IO) set_stop(j, STOP_OTHER, 1, "\"%s\": %s", x.name, why);
        else
            set_stop(j, STOP_VERIFY, 0, "verification failed: \"%s\" read back differently from what was written (%s). The storage may be failing; its original and every later file were kept.", x.name, why);
        zs_free(why);
        return 0;
    }
    for (i = 0; i < *np; i++) commit_record(&b, &pending[i]), journal_add(&j->jr, &b);
    buf_free(&b);
    if (zs_fail("before_journal_commit")) return stop_io(j, j->paths.journal, GetLastError()), 0;
    if (!jsync(j)) return 0;
    /* From here on these entries are committed: never cut them off. */
    for (i = 0; i < *np; i++) {
        const Commit *c = &pending[i];
        zs_free(j->failed[c->idx]);
        j->failed[c->idx] = NULL;
        if (j->plan.e[c->idx].is_dir) j->st.dirs_archived++;
        else j->st.files_archived++, j->st.bytes_committed += c->usize, j->st.compressed_committed += c->csize;
        push_commit(j, c);
        status(j, c->idx, ST_ARCHIVED, NULL);
    }
    *np = 0;
    if (zs_fail("journal_committed")) return stop_io(j, j->paths.journal, GetLastError()), 0;
    if (j->opt.delete_originals) {
        for (i = first; i < j->ncommits; i++) {
            Commit c = j->commits[i];
            if (j->plan.e[c.idx].is_dir) continue;
            if (!delete_original(j, &c, !j->opt.quick_check)) return 0;
        }
    }
    if (!jsync(j)) return 0;
    tick(j, 1);
    return 1;
}

static int resume_pending_deletions(Job *j) {
    size_t i;
    if (!j->opt.delete_originals) return 1;
    for (i = 0; i < j->ncommits; i++) {
        Commit c = j->commits[i];
        Expected x;
        Readback mode;
        char *why = NULL;
        if (j->plan.e[c.idx].is_dir || j->del[c.idx] != DEL_NONE) continue;
        if (j->cancel && *j->cancel) return set_stop(j, STOP_CANCELLED, 1, "stopped at your request"), 0;
        status(j, c.idx, ST_VERIFYING, NULL);
        x = expected_of(j, &c);
        if (check_entry(j->arch, j->paths.partial, &x, &mode, &why) != CHK_OK) {
            set_stop(j, STOP_DAMAGED, 0,
                     "the partial archive is damaged, so no further originals will be deleted: the archived copy of \"%s\" failed verification (%s). Its original was kept. Keep the .partial and .journal files and run \"zipshrink salvage\".",
                     x.name, why ? why : "?");
            zs_free(why);
            return 0;
        }
        /* After an interruption always compare full contents. */
        if (!delete_original(j, &c, 1)) return 0;
        tick(j, 0);
    }
    return jsync(j);
}

static int compress_all(Job *j) {
    Commit *pending = zs_alloc(sizeof(Commit) * (BATCH_ENTRIES + 1));
    size_t np = 0, i;
    u64 pending_bytes = 0;
    u32 batch_started = zs_ticks();
    size_t batch = j->opt.batch_entries ? j->opt.batch_entries : BATCH_ENTRIES;
    int ok = 1;
    if (batch > BATCH_ENTRIES) batch = BATCH_ENTRIES;
    for (i = 0; i < j->plan.n && ok; i++) {
        const PlanEntry *e = &j->plan.e[i];
        int attempt = 0, retried_flush = 0;
        WResult r;
        Commit c;
        char *why = NULL;
        if (j->at[i] >= 0) continue;
        if (j->cancel && *j->cancel) {
            set_stop(j, STOP_CANCELLED, 1, "stopped at your request");
            ok = 0;
            break;
        }
        {
            size_t n = strlen(e->name);
            if (n >= sizeof(j->st.cur_name)) n = sizeof(j->st.cur_name) - 1;
            memcpy(j->st.cur_name, e->name, n);
            j->st.cur_name[n] = 0;
        }
        j->st.has_current = 1, j->st.cur_size = e->size, j->st.cur_done = 0, j->st.cur_status = ST_COMPRESSING;
        status(j, i, ST_COMPRESSING, NULL);
        for (;;) {
            u64 start = j->arch_len;
            zs_free(why), why = NULL;
            if (!ensure_space(j, 64 << 10)) r = W_STOP;
            else r = e->is_dir ? write_dir(j, e, &c) : write_file(j, e, &c, &why);
            if (r != W_OK && j->arch_len != start) cut_back(j, start);
            if ((r == W_CHANGED) && ++attempt < CHANGE_RETRIES) continue;
            if (r == W_STOP && (j->stop == STOP_LOWSPACE || j->stop == STOP_DISKFULL) && np && !retried_flush) {
                /* Committing (and deleting) the batch may free space. */
                j->stop = STOP_NONE, zs_free(j->error), j->error = NULL;
                if (!flush_batch(j, pending, &np)) {
                    r = W_STOP;
                    break;
                }
                pending_bytes = 0, batch_started = zs_ticks(), retried_flush = 1;
                continue;
            }
            break;
        }
        if (r == W_OK) {
            pending[np++] = c;
            pending_bytes += c.usize;
        } else if (r == W_SOURCE || r == W_CHANGED) {
            mark_failed(j, i, why ? why : zs_strdup("could not be read"));
            why = NULL;
        } else {
            ok = 0;
        }
        zs_free(why);
        if (ok && (pending_bytes >= BATCH_BYTES || np >= batch || zs_ticks() - batch_started >= BATCH_MS)) {
            if (!flush_batch(j, pending, &np)) ok = 0;
            pending_bytes = 0, batch_started = zs_ticks();
        }
        tick(j, 0);
    }
    if (ok && !flush_batch(j, pending, &np)) ok = 0;
    j->st.has_current = 0;
    if (!ok) journal_sync(&j->jr); /* records already earned; the tail is cut on resume */
    zs_free(pending);
    return ok;
}

static void verify_cb(void *ctx, u64 n) {
    Job *j = ctx;
    j->st.verify_done += n;
    tick(j, 0);
}

static int cross_check(Job *j, const char *path) {
    HANDLE h = plat_open_read(path, 0);
    CdEntry *cd;
    size_t n, i;
    char *err = NULL;
    if (h == INVALID_HANDLE_VALUE) return set_stop(j, STOP_VERIFY, 0, "the finished archive could not be opened: %s", plat_error(GetLastError())), 0;
    if (!zip_read_cd(h, &cd, &n, &err)) {
        CloseHandle(h);
        set_stop(j, STOP_VERIFY, 0, "verification failed: %s", err);
        zs_free(err);
        return 0;
    }
    CloseHandle(h);
    if (n != j->ncommits) {
        zip_free_cd(cd, n);
        return set_stop(j, STOP_VERIFY, 0, "verification failed: the archive lists %U entries but %U were committed", (u64)n, (u64)j->ncommits), 0;
    }
    for (i = 0; i < n; i++) {
        const Commit *c = &j->commits[i];
        if (zs_strcmp(cd[i].name, j->plan.e[c->idx].name) || cd[i].crc != c->crc || cd[i].csize != c->csize || cd[i].usize != c->usize ||
            cd[i].offset != c->offset || cd[i].has_sha != c->has_sha || (c->has_sha && memcmp(cd[i].sha, c->sha, 32))) {
            set_stop(j, STOP_VERIFY, 0, "verification failed: the central directory entry for \"%s\" does not match the journal", j->plan.e[c->idx].name);
            zip_free_cd(cd, n);
            return 0;
        }
    }
    zip_free_cd(cd, n);
    return 1;
}

static int finalize(Job *j) {
    u64 cd_off = j->ncommits ? j->commits[j->ncommits - 1].end : 0, cd_size;
    Buf b = {0}, cd = {0};
    size_t i;
    char *err = NULL;
    if (j->arch_len != cd_off) cut_back(j, cd_off);
    rec_begin(&b, 'S');
    rec_num(&b, "cd", cd_off);
    rec_num(&b, "n", j->ncommits);
    journal_add(&j->jr, &b);
    if (!jsync(j)) return buf_free(&b), 0;
    for (i = 0; i < j->ncommits; i++) {
        EntryMeta m;
        EntrySizes s;
        meta_of(j, &j->commits[i], &m);
        sizes_of(&j->commits[i], &s);
        zip_central_header(&cd, &m, &s, j->commits[i].offset);
        if (cd.len >= IO_BUF || i + 1 == j->ncommits) {
            if (!arch_append(j, cd.p, (u32)cd.len)) return buf_free(&b), buf_free(&cd), stop_io(j, j->paths.partial, GetLastError()), 0;
            cd.len = 0;
            if (i + 1 < j->ncommits && zs_fail("finalize_cd_partial")) return buf_free(&b), buf_free(&cd), stop_io(j, j->paths.partial, GetLastError()), 0;
        }
    }
    cd_size = j->arch_len - cd_off;
    if (zs_fail("finalize_cd_partial")) return buf_free(&b), buf_free(&cd), stop_io(j, j->paths.partial, GetLastError()), 0;
    cd.len = 0;
    zip_end_records(&cd, j->ncommits, cd_off, cd_size);
    if (!arch_append(j, cd.p, (u32)cd.len) || zs_fail("finalize_written") || !plat_flush(j->arch)) {
        buf_free(&b), buf_free(&cd);
        return stop_io(j, j->paths.partial, GetLastError()), 0;
    }
    buf_free(&cd);
    j->st.archive_size = j->arch_len;
    set_phase(j, PH_VERIFY);
    j->st.verify_total = j->opt.final_verify ? j->st.bytes_committed : 0;
    j->st.verify_done = 0;
    archive_check_free(&j->check);
    if (!check_archive(j->paths.partial, j->opt.final_verify, j->cancel, verify_cb, j, &j->check, &err)) {
        if (j->cancel && *j->cancel) set_stop(j, STOP_CANCELLED, 1, "stopped at your request");
        else set_stop(j, STOP_VERIFY, 0, "verification failed: the finished archive could not be read back: %s", err);
        zs_free(err);
        buf_free(&b);
        return 0;
    }
    if (!archive_ok(&j->check)) {
        set_stop(j, STOP_VERIFY, 0, "verification failed: %s", (char *)j->check.problems.v[0]);
        buf_free(&b);
        return 0;
    }
    if (!cross_check(j, j->paths.partial)) return buf_free(&b), 0;
    j->verified = 1;
    if (zs_fail("before_rename")) return buf_free(&b), stop_io(j, j->paths.partial, GetLastError()), 0;
    CloseHandle(j->arch);
    j->arch = INVALID_HANDLE_VALUE;
    if (plat_exists(j->paths.output)) {
        if (!j->opt.overwrite) {
            buf_free(&b);
            set_stop(j, STOP_OTHER, 1, "%s already exists. Move it away, then resume.", j->paths.output);
            return 0;
        }
        if (!plat_rename_replace(j->paths.partial, j->paths.output)) return buf_free(&b), stop_io(j, j->paths.output, GetLastError()), 0;
    } else if (!plat_rename_noreplace(j->paths.partial, j->paths.output)) {
        return buf_free(&b), stop_io(j, j->paths.output, GetLastError()), 0;
    }
    if (zs_fail("after_rename")) return buf_free(&b), stop_io(j, j->paths.output, GetLastError()), 0;
    rec_begin(&b, 'Z');
    rec_str(&b, "path", j->paths.output);
    journal_add(&j->jr, &b);
    buf_free(&b);
    if (!jsync(j)) return 0;
    j->finalized = 1;
    return 1;
}

static int cleanup(Job *j) {
    size_t i;
    if (zs_fail("before_cleanup")) return stop_io(j, j->paths.journal, GetLastError()), 0;
    if (j->opt.delete_originals) {
        for (i = j->plan.n; i-- > 0;) {
            const PlanEntry *e = &j->plan.e[i];
            if (!e->is_dir || j->at[i] < 0 || !path_within(e->src, j->sources[e->root])) continue;
            if (plat_remove_dir(e->src)) j->dirs_removed++;
        }
    }
    journal_close(&j->jr);
    if (!plat_delete_file(j->paths.journal)) message(j, "the journal could not be removed; it is no longer needed");
    return 1;
}

static int run_inner(Job *j) {
    if (!j->finalized) {
        if (j->resumed) {
            set_phase(j, PH_RECOVER);
            if (!resume_pending_deletions(j)) return 0;
        }
        set_phase(j, PH_COMPRESS);
        if (!compress_all(j)) return 0;
        set_phase(j, PH_FINALIZE);
        if (!finalize(j)) return 0;
    }
    set_phase(j, PH_CLEANUP);
    return cleanup(j);
}

static void dup_paths(JobPaths *d, const JobPaths *s) {
    d->output = zs_strdup(s->output), d->partial = zs_strdup(s->partial), d->journal = zs_strdup(s->journal);
}

Report *job_run(Job *j, Observer *obs, volatile LONG *cancel) {
    Report *r = zs_zalloc(sizeof(Report));
    size_t i;
    int ok;
    j->obs = obs, j->cancel = cancel;
    j->started = j->last_tick = j->speed_t = zs_ticks() - 1000;
    j->started += 1000;
    j->speed_b = j->st.bytes_processed;
    ok = run_inner(j);
    if (ok) j->st.phase = PH_DONE;
    tick(j, 1);
    dup_paths(&r->paths, &j->paths);
    r->delete_requested = j->opt.delete_originals;
    r->elapsed_ms = zs_ticks() - j->started;
    for (i = 0; i < j->plan.n; i++) {
        if (j->failed[i]) {
            r->failed = zs_realloc(r->failed, (r->nfailed + 1) * sizeof(Note));
            r->failed[r->nfailed].path = zs_strdup(j->plan.e[i].src);
            r->failed[r->nfailed].reason = zs_strdup(j->failed[i]);
            r->nfailed++;
        }
        if (j->del[i] == DEL_WASKEPT && j->at[i] >= 0) {
            r->kept = zs_realloc(r->kept, (r->nkept + 1) * sizeof(Note));
            r->kept[r->nkept].path = zs_strdup(j->plan.e[i].name);
            r->kept[r->nkept].reason = zs_strdup(j->del_why[i] ? j->del_why[i] : "kept");
            r->nkept++;
        }
    }
    for (i = 0; i < j->plan.nskips; i++) scan_note(&r->skipped, &r->nskipped, j->plan.skips[i].path, j->plan.skips[i].reason);
    for (i = 0; i < j->plan.nwarnings; i++) pv_push(&r->warnings, zs_strdup(j->plan.warnings[i]));
    r->original_bytes = j->st.bytes_committed;
    r->reclaimed = j->st.reclaimed;
    r->files_archived = j->st.files_archived;
    r->dirs_archived = j->st.dirs_archived;
    r->deleted = j->st.deleted;
    r->dirs_removed = j->dirs_removed;
    r->uncached_readback = j->any_readback && j->all_uncached;
    if (j->verified) r->verified = 1, r->check = j->check, memset(&j->check, 0, sizeof(j->check));
    if (ok) {
        int issues = r->nfailed || r->nskipped || r->nkept || j->ncommits < j->plan.n;
        r->outcome = issues ? OUT_ISSUES : OUT_COMPLETED;
        {
            HANDLE h = plat_open_read(j->paths.output, 0);
            if (h != INVALID_HANDLE_VALUE) r->archive_size = plat_file_len(h), CloseHandle(h);
        }
    } else {
        r->outcome = OUT_STOPPED;
        r->stop = j->stop == STOP_NONE ? STOP_OTHER : j->stop;
        r->resumable = j->stop == STOP_NONE ? 1 : j->resumable;
        r->error = zs_strdup(j->error ? j->error : "the job stopped");
        r->archive_size = j->arch_len;
        r->recoverable = j->ncommits;
    }
    return r;
}

void report_free(Report *r) {
    size_t i;
    if (!r) return;
    for (i = 0; i < r->nkept; i++) zs_free(r->kept[i].path), zs_free(r->kept[i].reason);
    for (i = 0; i < r->nfailed; i++) zs_free(r->failed[i].path), zs_free(r->failed[i].reason);
    for (i = 0; i < r->nskipped; i++) zs_free(r->skipped[i].path), zs_free(r->skipped[i].reason);
    zs_free(r->kept), zs_free(r->failed), zs_free(r->skipped);
    pv_free(&r->warnings, 1);
    archive_check_free(&r->check);
    zs_free(r->error);
    paths_free(&r->paths);
    zs_free(r);
}

int jobs_create_parts(const Options *o, char **roots, size_t nroots, Part *parts, int nparts, Job ***out, char **err, ErrKind *kind) {
    Job **jobs;
    int k;
    for (k = 0; k < nparts; k++) {
        JobPaths p;
        paths_for_output(&p, parts[k].output);
        if (paths_unfinished(&p)) {
            *kind = ERR_JOB_EXISTS;
            *err = zs_fmt("an unfinished job for %s already exists. Resume it, or move it away first.", parts[k].output);
            paths_free(&p);
            return 0;
        }
        if (plat_exists(p.output) && !o->overwrite) {
            *kind = ERR_EXISTS;
            *err = zs_fmt("%s already exists. Choose another name, or allow overwriting it.", p.output);
            paths_free(&p);
            return 0;
        }
        paths_free(&p);
    }
    jobs = zs_zalloc((size_t)nparts * sizeof(Job *));
    for (k = 0; k < nparts; k++) {
        jobs[k] = job_create(o, roots, nroots, &parts[k].scan, parts[k].output, err, kind);
        if (!jobs[k]) {
            int m;
            for (m = 0; m < k; m++) job_free(jobs[m]);
            zs_free(jobs);
            return 0;
        }
    }
    *out = jobs;
    return 1;
}
