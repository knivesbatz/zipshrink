/* The job: plan, compress, verify, commit, delete, finalize, resume.
 *
 * For each batch of entries the engine (1) reads each source once through a
 * handle that keeps writers out, computing CRC-32 and SHA-256 while
 * compressing it onto NAME.zip.partial; (2) flushes the partial archive;
 * (3) reads every entry back (uncached where Windows allows), parses its
 * header, inflates it and matches both checksums; (4) commits the batch to
 * the journal and flushes it; (5) only then, if deletion is on, re-checks and
 * deletes each original. See docs/DATA_SAFETY.md. */
#ifndef ZS_ENGINE_H
#define ZS_ENGINE_H
#include "base.h"
#include "plat.h"
#include "zipfmt.h"

#define DEFAULT_MIN_FREE (256ull << 20)
#define MIN_FREE_FLOOR (16ull << 20)

typedef struct {
    int level; /* 0 store, 1-9 deflate */
    int delete_originals;
    int delete_readonly;
    u64 min_free;
    int quick_check; /* compare metadata only before deleting */
    int overwrite;
    int final_verify; /* decompress every entry again at the end */
    u32 batch_entries;
    int force_zip64; /* tests */
} Options;
void options_default(Options *o);

typedef enum { PH_SCAN, PH_RECOVER, PH_COMPRESS, PH_FINALIZE, PH_VERIFY, PH_CLEANUP, PH_DONE } Phase;
const char *phase_label(Phase p);

typedef enum { ST_WAITING, ST_COMPRESSING, ST_VERIFYING, ST_ARCHIVED, ST_DELETED, ST_KEPT, ST_SKIPPED, ST_FAILED } Status;
const char *status_label(Status s);

typedef struct {
    Phase phase;
    u64 total_files, total_dirs, total_bytes;
    u64 files_archived, dirs_archived, deleted, kept, failed, skipped;
    u64 bytes_processed, bytes_committed, compressed_committed;
    u64 archive_size, reclaimed, free_space;
    u64 speed; /* bytes per second */
    u32 elapsed_ms;
    int delete_originals;
    u64 verify_done, verify_total;
    int has_current;
    char cur_name[260];
    u64 cur_size, cur_done;
    Status cur_status;
} Stats;
u32 stats_permille(const Stats *s); /* 0..1000 */
u64 stats_remaining_files(const Stats *s);

typedef struct {
    void *ctx;
    void (*phase)(void *ctx, Phase p);
    void (*entry)(void *ctx, u64 idx, const char *name, u64 size, Status st, const char *detail);
    void (*progress)(void *ctx, const Stats *s);
    void (*message)(void *ctx, const char *text);
} Observer;

/* ---- planning ---- */
typedef struct {
    u64 idx;
    char *src;  /* full path */
    u32 root;   /* index into the roots */
    char *name; /* archive name */
    int is_dir;
    u64 size, mtime, atime, btime;
    u32 attrs;
} PlanEntry;

typedef struct {
    char *path, *reason;
} Note;

typedef struct {
    char **roots;
    size_t nroots;
    PlanEntry *e;
    size_t n, cap;
    Note *skips;
    size_t nskips;
    char **warnings;
    size_t nwarnings;
    u64 files, dirs, bytes, largest;
} Scan;

/* Validates and makes the selection absolute; *err on failure. */
int scan_roots(const char *const *sources, size_t n, char ***roots, size_t *nroots, char **err);
/* Walks the roots (no links followed) into the plan. */
int scan_build(Scan *s, char **roots, size_t nroots, const char *const *exclude, size_t nexclude, volatile LONG *cancel, char **err);
void scan_free(Scan *s);
void scan_add(Scan *s, const PlanEntry *e); /* copies strings */
void scan_note(Note **v, size_t *n, const char *path, const char *reason);
int path_within(const char *p, const char *root);

typedef struct {
    char *output, *partial, *journal;
} JobPaths;
void paths_for_output(JobPaths *p, const char *output);
void paths_from_any(JobPaths *p, const char *any);
void paths_free(JobPaths *p);
int paths_unfinished(const JobPaths *p);

/* ---- disk space ---- */
typedef enum { V_FITS, V_PROBABLY, V_NO } Verdict;
typedef struct {
    u64 files, dirs, bytes, largest;
    u64 estimated, worst;
    u32 ratio_permille; /* sampled compressed/original, 1000 if unknown */
    char *dest_dir;
    u64 free_space, min_free, same_volume_bytes;
    int delete_originals;
    u64 need_worst, need_estimate;
    char *fs_name;
    Verdict verdict;
    char *explanation;
    PtrVec warnings;
} Preflight;
u32 sample_ratio(const Scan *s, int level);
int preflight(const Scan *s, const JobPaths *paths, const Options *o, Preflight *out, char **err);
void preflight_free(Preflight *p);

/* ---- split ---- */
typedef struct {
    int number;
    char *output;
    Scan scan;
    u64 files, bytes, estimated, worst;
} Part;
char *part_path(const char *output, int number, int parts);
#define ZS_MAX_PARTS 999
#define ZS_MIN_PART_SIZE (64 * 1024ULL)
int split_scan(const Scan *s, const char *output, int parts, u32 ratio_permille, Part **out, char **err);
/* Parts whose estimated size stays within `size` (a file larger than that
 * gets a part of its own). *nparts is 1, with no parts, when everything
 * fits in one ZIP file. */
int split_scan_size(const Scan *s, const char *output, u64 size, u32 ratio_permille, Part **out, int *nparts, char **err);
void parts_free(Part *p, int n);
/* Outputs of the unfinished parts of the set `path` belongs to, in order. */
PtrVec unfinished_parts(const char *path);
/* What resume on `path` continues: its own job, then the rest of its set. */
PtrVec resume_targets(const char *path);

/* ---- jobs ---- */
typedef enum { ERR_NONE, ERR_INVALID, ERR_IO, ERR_EXISTS, ERR_JOB_EXISTS, ERR_DAMAGED, ERR_BUSY, ERR_SPACE } ErrKind;
typedef enum { OUT_COMPLETED, OUT_ISSUES, OUT_STOPPED } Outcome;
typedef enum { STOP_NONE, STOP_CANCELLED, STOP_LOWSPACE, STOP_DISKFULL, STOP_DAMAGED, STOP_VERIFY, STOP_OTHER } StopKind;

typedef struct {
    u64 entries, files, dirs, sha_checked, crc_only, bytes;
    int data_checked;
    PtrVec problems; /* char* */
} ArchiveCheck;
int archive_ok(const ArchiveCheck *c);
void archive_check_free(ArchiveCheck *c);

typedef struct {
    Outcome outcome;
    StopKind stop;
    int resumable;
    char *error;
    JobPaths paths;
    u64 archive_size, original_bytes, reclaimed;
    u64 files_archived, dirs_archived, deleted, dirs_removed, recoverable;
    int delete_requested;
    Note *kept, *failed, *skipped;
    size_t nkept, nfailed, nskipped;
    PtrVec warnings;
    int verified; /* verification below is valid */
    ArchiveCheck check;
    int uncached_readback; /* every read-back bypassed the cache */
    u32 elapsed_ms;
} Report;
void report_free(Report *r);

typedef struct Job Job;
Job *job_create(const Options *o, char **roots, size_t nroots, Scan *scan, const char *output, char **err, ErrKind *kind);
Job *job_open(const char *path, char **err, ErrKind *kind);
void job_set_min_free(Job *j, u64 bytes);
void job_disable_delete(Job *j);
const Options *job_options(const Job *j);
const char *job_output(const Job *j);
const char *job_partial(const Job *j);
size_t job_nsources(const Job *j);
const char *job_source(const Job *j, size_t i);
Report *job_run(Job *j, Observer *obs, volatile LONG *cancel);
void job_free(Job *j);

/* Validates a request: absolute output path, roots, output not in a source. */
int job_validate(const char *const *sources, size_t n, const char *output, char **out_abs, char ***roots, size_t *nroots, char **err);
/* Creates one job per part, after checking none is in the way. */
int jobs_create_parts(const Options *o, char **roots, size_t nroots, Part *parts, int nparts, Job ***jobs, char **err, ErrKind *kind);

/* ---- verification ---- */
typedef void (*BytesCb)(void *ctx, u64 n);
int check_archive(const char *path, int data, volatile LONG *cancel, BytesCb cb, void *ctx, ArchiveCheck *out, char **err);

typedef struct {
    char *output;
    u64 entries, files, dirs, bytes;
    u64 damaged_ranges;
    ArchiveCheck check;
} SalvageReport;
char *salvage_default_output(const char *partial);
int salvage(const char *partial, const char *output, volatile LONG *cancel, SalvageReport *out, char **err, ErrKind *kind);
#endif
