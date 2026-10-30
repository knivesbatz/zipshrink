/* Splitting one selection into several complete, independent ZIP files,
 * either into N parts or into parts of a given size. Files are assigned in
 * plan order and a file is never cut in two. Each part repeats the folder
 * entries above its files.
 *   N parts: cut where the running total reaches k/N of the data while every
 *            later part keeps at least one file.
 *   size:    start a new part when the next file would take the current
 *            part's estimated size over the limit. */
#include "engine.h"

char *part_path(const char *output, int number, int parts) {
    const char *base = zs_basename(output), *dot = NULL, *p;
    char *dir = zs_strndup(output, (size_t)(base - output));
    char *stem, *res;
    int width = parts >= 100 ? 3 : parts >= 10 ? 2 : 1;
    for (p = base; *p; p++)
        if (*p == '.' && p != base) dot = p;
    stem = dot ? zs_strndup(base, (size_t)(dot - base)) : zs_strdup(base);
    res = width == 3 ? zs_fmt("%s%s.part%03d%s", dir, stem, number, dot ? dot : "")
          : width == 2 ? zs_fmt("%s%s.part%02d%s", dir, stem, number, dot ? dot : "")
                       : zs_fmt("%s%s.part%d%s", dir, stem, number, dot ? dot : "");
    zs_free(dir);
    zs_free(stem);
    return res;
}

static u64 est(u64 n, u32 r) { return n / 1000 * r + (n % 1000) * r / 1000; }

/* Folder name -> plan index, open addressing (FNV-1a). */
typedef struct {
    size_t *slot; /* index + 1, 0 = empty */
    size_t cap;
} DirIndex;

static u32 fnv(const char *s, size_t n) {
    u32 h = 2166136261u;
    while (n--) h = (h ^ (u8)*s++) * 16777619u;
    return h;
}

static void dir_index_build(DirIndex *ix, const Scan *s) {
    size_t i;
    ix->cap = 16;
    while (ix->cap < s->n * 2 + 2) ix->cap *= 2;
    ix->slot = zs_zalloc(ix->cap * sizeof(size_t));
    for (i = 0; i < s->n; i++) {
        size_t h;
        if (!s->e[i].is_dir) continue;
        h = fnv(s->e[i].name, strlen(s->e[i].name)) & (ix->cap - 1);
        while (ix->slot[h]) h = (h + 1) & (ix->cap - 1);
        ix->slot[h] = i + 1;
    }
}

static long dir_index_find(const DirIndex *ix, const Scan *s, const char *name, size_t n) {
    size_t h = fnv(name, n) & (ix->cap - 1);
    while (ix->slot[h]) {
        const char *c = s->e[ix->slot[h] - 1].name;
        if (strlen(c) == n && !zs_strncmp(c, name, n)) return (long)(ix->slot[h] - 1);
        h = (h + 1) & (ix->cap - 1);
    }
    return -1;
}

static u64 entry_overhead(size_t name_len) { return 2 * (u64)name_len + 230; }

/* Parts from a per-entry part number (files set; folders filled in here). */
static void build_parts(const Scan *s, const char *output, int parts, int *assign, u32 ratio, Part **out) {
    size_t i;
    int k;
    u8 **member;
    Part *v;
    DirIndex ix;
    /* A folder goes with the next file after it. */
    {
        int next = parts - 1;
        for (i = s->n; i-- > 0;) {
            if (s->e[i].is_dir) assign[i] = next;
            else next = assign[i];
        }
    }
    dir_index_build(&ix, s);
    member = zs_alloc((size_t)parts * sizeof(u8 *));
    for (k = 0; k < parts; k++) member[k] = zs_zalloc(s->n);
    for (i = 0; i < s->n; i++) {
        const char *name = s->e[i].name;
        size_t len = strlen(name), cut;
        member[assign[i]][i] = 1;
        if (len && name[len - 1] == '/') len--;
        /* Parent folders, so every part extracts into the same tree. */
        for (cut = len; cut > 0; cut--) {
            if (name[cut - 1] == '/') {
                long j = dir_index_find(&ix, s, name, cut);
                if (j >= 0) member[assign[i]][j] = 1;
            }
        }
    }
    v = zs_zalloc((size_t)parts * sizeof(Part));
    for (k = 0; k < parts; k++) {
        Part *pt = &v[k];
        u64 ovh = 200;
        pt->number = k + 1;
        pt->output = part_path(output, k + 1, parts);
        pt->scan.roots = zs_alloc(s->nroots * sizeof(char *));
        for (i = 0; i < s->nroots; i++) pt->scan.roots[i] = zs_strdup(s->roots[i]);
        pt->scan.nroots = s->nroots;
        if (k == 0) {
            for (i = 0; i < s->nskips; i++) scan_note(&pt->scan.skips, &pt->scan.nskips, s->skips[i].path, s->skips[i].reason);
            for (i = 0; i < s->nwarnings; i++) {
                pt->scan.warnings = zs_realloc(pt->scan.warnings, (pt->scan.nwarnings + 1) * sizeof(char *));
                pt->scan.warnings[pt->scan.nwarnings++] = zs_strdup(s->warnings[i]);
            }
        }
        for (i = 0; i < s->n; i++) {
            if (!member[k][i]) continue;
            scan_add(&pt->scan, &s->e[i]);
            ovh += entry_overhead(strlen(s->e[i].name));
            if (s->e[i].is_dir) pt->scan.dirs++;
            else {
                pt->scan.files++, pt->scan.bytes += s->e[i].size;
                if (s->e[i].size > pt->scan.largest) pt->scan.largest = s->e[i].size;
            }
        }
        pt->files = pt->scan.files, pt->bytes = pt->scan.bytes;
        pt->estimated = est(pt->bytes, ratio) + ovh;
        pt->worst = zip_worst(pt->bytes) + ovh;
        zs_free(member[k]);
    }
    zs_free(member);
    zs_free(ix.slot);
    *out = v;
}

int split_scan(const Scan *s, const char *output, int parts, u32 ratio, Part **out, char **err) {
    size_t i, files = 0, seen = 0, in_part = 0;
    int part = 0;
    int *assign;
    u64 acc = 0, total = s->bytes ? s->bytes : 1;
    if (parts < 2 || parts > ZS_MAX_PARTS) return *err = zs_fmt("a split needs 2 to %d parts", ZS_MAX_PARTS), 0;
    for (i = 0; i < s->n; i++)
        if (!s->e[i].is_dir) files++;
    if (files < (size_t)parts) return *err = zs_fmt("the selection has %u files, so it cannot be split into %d parts", (unsigned)files, parts), 0;
    assign = zs_zalloc(s->n * sizeof(int));
    for (i = 0; i < s->n; i++) {
        if (!s->e[i].is_dir) {
            size_t remaining = files - seen; /* including this one */
            u64 boundary = total / (u64)parts * (u64)(part + 1) + total % (u64)parts * (u64)(part + 1) / (u64)parts;
            int must_cut = remaining <= (size_t)(parts - part - 1);
            if (part + 1 < parts && in_part > 0 && (acc >= boundary || must_cut)) part++, in_part = 0;
            acc += s->e[i].size, seen++, in_part++;
        }
        assign[i] = part;
    }
    build_parts(s, output, parts, assign, ratio, out);
    zs_free(assign);
    return 1;
}

int split_scan_size(const Scan *s, const char *output, u64 size, u32 ratio, Part **out, int *nparts, char **err) {
    size_t i, in_part = 0;
    int part = 0;
    int *assign;
    u64 cur = 200;
    *out = NULL, *nparts = 1;
    if (size < ZS_MIN_PART_SIZE) return *err = zs_strdup("the size of each ZIP file should be at least 64 KB"), 0;
    assign = zs_zalloc(s->n * sizeof(int));
    for (i = 0; i < s->n; i++) {
        const char *name = s->e[i].name;
        size_t len = strlen(name);
        u64 add = entry_overhead(len);
        if (!s->e[i].is_dir) {
            add += est(s->e[i].size, ratio);
            if (in_part > 0 && cur + add > size) {
                size_t cut;
                if (++part >= ZS_MAX_PARTS) {
                    zs_free(assign);
                    return *err = zs_fmt("this would make more than %d ZIP files; choose a larger size", ZS_MAX_PARTS), 0;
                }
                in_part = 0, cur = 200;
                /* The new part repeats the folders above this file. */
                for (cut = len; cut > 0; cut--)
                    if (name[cut - 1] == '/') cur += entry_overhead(cut);
            }
            in_part++;
        }
        cur += add;
        assign[i] = part;
    }
    if (part > 0) {
        *nparts = part + 1;
        build_parts(s, output, *nparts, assign, ratio, out);
    }
    zs_free(assign);
    return 1;
}

void parts_free(Part *p, int n) {
    int k;
    for (k = 0; k < n; k++) zs_free(p[k].output), scan_free(&p[k].scan);
    zs_free(p);
}

/* "Games.part02.zip" -> stem "Games", ext ".zip", number 2. */
static int parse_part(const char *name, char **stem, char **ext, int *num) {
    const char *p = NULL, *q;
    for (q = name; *q; q++)
        if (!zs_strncmp(q, ".part", 5)) p = q;
    if (!p) return 0;
    q = p + 5;
    if (*q < '0' || *q > '9') return 0;
    *num = 0;
    while (*q >= '0' && *q <= '9') *num = *num * 10 + (*q++ - '0');
    *stem = zs_strndup(name, (size_t)(p - name));
    *ext = zs_strdup(q);
    return 1;
}

PtrVec unfinished_parts(const char *path) {
    PtrVec res = {0};
    JobPaths jp;
    char *dir, *stem = NULL, *ext = NULL;
    const char *name;
    int num;
    DirEntry *list;
    size_t n, i;
    int nums[1000];
    char *found[1000];
    size_t nf = 0;
    paths_from_any(&jp, path);
    dir = zs_dirname(jp.output);
    name = zs_basename(jp.output);
    if (!parse_part(name, &stem, &ext, &num)) {
        const char *dot = NULL, *p;
        for (p = name; *p; p++)
            if (*p == '.' && p != name) dot = p;
        stem = dot ? zs_strndup(name, (size_t)(dot - name)) : zs_strdup(name);
        ext = zs_strdup(dot ? dot : "");
    }
    if (plat_list_dir(dir, &list, &n)) {
        for (i = 0; i < n && nf < 1000; i++) {
            char *s2, *e2, *base;
            int k;
            if (!list[i].name || !zs_ends_with_i(list[i].name, ".partial.journal")) continue;
            base = zs_strndup(list[i].name, strlen(list[i].name) - 16);
            if (parse_part(base, &s2, &e2, &k)) {
                if (!zs_stricmp(s2, stem) && !zs_stricmp(e2, ext)) {
                    nums[nf] = k;
                    found[nf++] = zs_path_join(dir, base);
                }
                zs_free(s2), zs_free(e2);
            }
            zs_free(base);
        }
        plat_free_entries(list, n);
    }
    /* Sort by part number. */
    for (i = 1; i < nf; i++) {
        size_t j = i;
        while (j > 0 && nums[j - 1] > nums[j]) {
            int t = nums[j];
            char *tp = found[j];
            nums[j] = nums[j - 1], found[j] = found[j - 1];
            nums[j - 1] = t, found[j - 1] = tp;
            j--;
        }
    }
    for (i = 0; i < nf; i++) pv_push(&res, found[i]);
    zs_free(dir), zs_free(stem), zs_free(ext);
    paths_free(&jp);
    return res;
}

PtrVec resume_targets(const char *path) {
    PtrVec res = {0}, parts;
    JobPaths jp;
    size_t i;
    paths_from_any(&jp, path);
    if (paths_unfinished(&jp)) pv_push(&res, zs_strdup(jp.output));
    parts = unfinished_parts(path);
    for (i = 0; i < parts.len; i++) {
        size_t k;
        int dup = 0;
        for (k = 0; k < res.len; k++)
            if (!zs_stricmp(res.v[k], parts.v[i])) dup = 1;
        if (dup) zs_free(parts.v[i]);
        else pv_push(&res, parts.v[i]);
    }
    zs_free(parts.v);
    if (!res.len) pv_push(&res, zs_strdup(jp.output));
    paths_free(&jp);
    return res;
}
