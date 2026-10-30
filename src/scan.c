/* Planning: the selection becomes an ordered list of entries. Links,
 * junctions and other reparse points are recorded as skipped and never
 * followed, so nothing outside the selected folders is read or deleted. */
#include "engine.h"

void scan_note(Note **v, size_t *n, const char *path, const char *reason) {
    *v = zs_realloc(*v, (*n + 1) * sizeof(Note));
    (*v)[*n].path = zs_strdup(path);
    (*v)[*n].reason = zs_strdup(reason);
    (*n)++;
}

void scan_add(Scan *s, const PlanEntry *e) {
    PlanEntry *d;
    if (s->n == s->cap) s->cap = s->cap ? s->cap * 2 : 256, s->e = zs_realloc(s->e, s->cap * sizeof(PlanEntry));
    d = &s->e[s->n];
    *d = *e;
    d->idx = s->n;
    d->src = zs_strdup(e->src);
    d->name = zs_strdup(e->name);
    s->n++;
}

void scan_free(Scan *s) {
    size_t i;
    for (i = 0; i < s->n; i++) zs_free(s->e[i].src), zs_free(s->e[i].name);
    zs_free(s->e);
    for (i = 0; i < s->nskips; i++) zs_free(s->skips[i].path), zs_free(s->skips[i].reason);
    zs_free(s->skips);
    for (i = 0; i < s->nwarnings; i++) zs_free(s->warnings[i]);
    zs_free(s->warnings);
    for (i = 0; i < s->nroots; i++) zs_free(s->roots[i]);
    zs_free(s->roots);
    memset(s, 0, sizeof(*s));
}

static int lowc(int c) {
    if (c >= 'A' && c <= 'Z') return c + 32;
    if (c == '/') return '\\';
    return c;
}

/* Case-insensitive (as Windows file systems are) path prefix test. */
int path_within(const char *p, const char *root) {
    size_t n = strlen(root);
    size_t i;
    while (n > 0 && (root[n - 1] == '\\' || root[n - 1] == '/')) n--;
    for (i = 0; i < n; i++)
        if (lowc((u8)p[i]) != lowc((u8)root[i])) return 0;
    return p[n] == 0 || p[n] == '\\' || p[n] == '/';
}

int scan_roots(const char *const *sources, size_t n, char ***roots, size_t *nroots, char **err) {
    size_t i, j;
    char **r;
    if (n == 0) return *err = zs_strdup("no files or folders were selected"), 0;
    r = zs_zalloc(n * sizeof(char *));
    for (i = 0; i < n; i++) {
        Kind k;
        FStat st;
        char *full = plat_full_path(sources[i]);
        size_t len;
        if (!full) {
            *err = zs_fmt("%s: %s", sources[i], plat_error(GetLastError()));
            goto fail;
        }
        len = strlen(full);
        while (len > 3 && (full[len - 1] == '\\' || full[len - 1] == '/')) full[--len] = 0;
        r[i] = full;
        if (!plat_kind(full, &k, &st) || k == K_MISSING) {
            *err = zs_fmt("%s does not exist", full);
            goto fail;
        }
        if (k == K_LINK) {
            *err = zs_fmt("%s is a symbolic link or junction; select the folder it points to instead", full);
            goto fail;
        }
        if (k == K_OTHER) {
            *err = zs_fmt("%s is an offline or cloud-only item and cannot be archived", full);
            goto fail;
        }
        if (len <= 3 || !*zs_basename(full)) {
            *err = zs_fmt("%s is the root of a drive; select folders inside it instead", full);
            goto fail;
        }
        for (j = 0; j < i; j++)
            if (path_within(full, r[j]) || path_within(r[j], full)) {
                *err = zs_fmt("%s and %s overlap; select each folder only once", r[j], full);
                goto fail;
            }
    }
    *roots = r, *nroots = n;
    return 1;
fail:
    for (j = 0; j < n; j++) zs_free(r[j]);
    zs_free(r);
    return 0;
}

static int name_used(PtrVec *used, const char *name) {
    size_t i;
    for (i = 0; i < used->len; i++)
        if (!zs_stricmp(used->v[i], name)) return 1;
    return 0;
}

static char *unique_top(PtrVec *used, const char *name, int is_dir) {
    const char *dot = is_dir ? NULL : NULL;
    const char *p;
    int k;
    if (!name_used(used, name)) return pv_push(used, zs_strdup(name)), zs_strdup(name);
    for (p = name; *p; p++)
        if (*p == '.' && p != name && !is_dir) dot = p;
    for (k = 2;; k++) {
        char *stem = dot ? zs_strndup(name, (size_t)(dot - name)) : zs_strdup(name);
        char *c = zs_fmt("%s (%d)%s", stem, k, dot ? dot : "");
        zs_free(stem);
        if (!name_used(used, c)) return pv_push(used, zs_strdup(c)), c;
        zs_free(c);
    }
}

static int excluded(const char *path, const char *const *ex, size_t n) {
    size_t i;
    for (i = 0; i < n; i++)
        if (!zs_stricmp(path, ex[i])) return 1;
    return 0;
}

typedef struct {
    char *path, *name;
    DirEntry st;
} Pending;

int scan_build(Scan *s, char **roots, size_t nroots, const char *const *exclude, size_t nex, volatile LONG *cancel, char **err) {
    PtrVec used = {0};
    size_t ri;
    memset(s, 0, sizeof(*s));
    s->roots = zs_alloc(nroots * sizeof(char *));
    for (ri = 0; ri < nroots; ri++) s->roots[ri] = zs_strdup(roots[ri]);
    s->nroots = nroots;
    for (ri = 0; ri < nroots; ri++) {
        Kind k;
        FStat st;
        const char *base = zs_basename(roots[ri]);
        char *top;
        PlanEntry e;
        plat_kind(roots[ri], &k, &st);
        top = unique_top(&used, base, k == K_DIR);
        if (zs_strcmp(top, base)) {
            char *w = zs_fmt("%s is stored as \"%s\" because another selected item has the same name", roots[ri], top);
            s->warnings = zs_realloc(s->warnings, (s->nwarnings + 1) * sizeof(char *));
            s->warnings[s->nwarnings++] = w;
        }
        memset(&e, 0, sizeof(e));
        e.root = (u32)ri, e.mtime = st.mtime, e.atime = st.atime, e.btime = st.btime, e.attrs = st.attrs;
        if (k != K_DIR) {
            e.src = roots[ri], e.name = top, e.size = st.size;
            scan_add(s, &e);
            s->files++, s->bytes += st.size;
            if (st.size > s->largest) s->largest = st.size;
            zs_free(top);
            continue;
        }
        {
            /* Depth first; a folder's files come before its subfolders. */
            Pending *stack = NULL;
            size_t sp = 0, cap = 0;
            stack = zs_alloc(sizeof(Pending) * (cap = 16));
            stack[sp].path = zs_strdup(roots[ri]);
            stack[sp].name = zs_join(top, "/");
            memset(&stack[sp].st, 0, sizeof(DirEntry));
            stack[sp].st.attrs = st.attrs, stack[sp].st.mtime = st.mtime, stack[sp].st.atime = st.atime, stack[sp].st.btime = st.btime;
            sp++;
            zs_free(top);
            while (sp) {
                Pending cur = stack[--sp];
                DirEntry *list = NULL;
                size_t nlist = 0, i, nsub = 0;
                Pending *subs = NULL;
                if (cancel && *cancel) {
                    zs_free(cur.path), zs_free(cur.name);
                    while (sp) sp--, zs_free(stack[sp].path), zs_free(stack[sp].name);
                    zs_free(stack);
                    pv_free(&used, 1);
                    return *err = zs_strdup("stopped at your request"), 0;
                }
                memset(&e, 0, sizeof(e));
                e.root = (u32)ri, e.src = cur.path, e.name = cur.name, e.is_dir = 1;
                e.mtime = cur.st.mtime, e.atime = cur.st.atime, e.btime = cur.st.btime, e.attrs = cur.st.attrs;
                scan_add(s, &e);
                s->dirs++;
                if (!plat_list_dir(cur.path, &list, &nlist)) scan_note(&s->skips, &s->nskips, cur.path, "part of this folder could not be read");
                for (i = 0; i < nlist; i++) {
                    DirEntry *d = &list[i];
                    char *child;
                    Kind ck;
                    if (!d->name) {
                        char *where = zs_path_join(cur.path, "(name not representable)");
                        scan_note(&s->skips, &s->nskips, where, "its name cannot be represented on this version of Windows (not archived, not deleted)");
                        zs_free(where);
                        continue;
                    }
                    child = zs_path_join(cur.path, d->name);
                    if (excluded(child, exclude, nex)) {
                        zs_free(child);
                        continue;
                    }
                    ck = (d->attrs & 0x400) ? K_LINK : (d->attrs & (0x1000 | 0x40000 | 0x400000)) ? K_OTHER : (d->attrs & FILE_ATTRIBUTE_DIRECTORY) ? K_DIR : K_FILE;
                    if (ck == K_LINK) scan_note(&s->skips, &s->nskips, child, "symbolic link, junction or reparse point (not followed, not deleted)");
                    else if (ck == K_OTHER) scan_note(&s->skips, &s->nskips, child, "offline or cloud-only file (not archived, not deleted)");
                    else if (ck == K_FILE) {
                        char *nm = zs_join(cur.name, d->name);
                        if (strlen(nm) > 65535) scan_note(&s->skips, &s->nskips, child, "its path is too long to store in a ZIP");
                        else {
                            memset(&e, 0, sizeof(e));
                            e.root = (u32)ri, e.src = child, e.name = nm, e.size = d->size;
                            e.mtime = d->mtime, e.atime = d->atime, e.btime = d->btime, e.attrs = d->attrs;
                            scan_add(s, &e);
                            s->files++, s->bytes += d->size;
                            if (d->size > s->largest) s->largest = d->size;
                        }
                        zs_free(nm);
                    } else {
                        subs = zs_realloc(subs, (nsub + 1) * sizeof(Pending));
                        subs[nsub].path = zs_strdup(child);
                        subs[nsub].name = zs_fmt("%s%s/", cur.name, d->name);
                        subs[nsub].st = *d;
                        subs[nsub].st.name = NULL;
                        nsub++;
                    }
                    zs_free(child);
                }
                plat_free_entries(list, nlist);
                /* Push in reverse so the first subfolder comes next. */
                while (nsub) {
                    if (sp == cap) stack = zs_realloc(stack, (cap *= 2) * sizeof(Pending));
                    stack[sp++] = subs[--nsub];
                }
                zs_free(subs);
                zs_free(cur.path);
                zs_free(cur.name);
            }
            zs_free(stack);
        }
    }
    pv_free(&used, 1);
    return 1;
}

void paths_for_output(JobPaths *p, const char *output) {
    p->output = zs_strdup(output);
    p->partial = zs_join(output, ".partial");
    p->journal = zs_join(p->partial, ".journal");
}

void paths_from_any(JobPaths *p, const char *any) {
    char *base;
    size_t n = strlen(any);
    if (zs_ends_with_i(any, ".partial.journal")) base = zs_strndup(any, n - 16);
    else if (zs_ends_with_i(any, ".partial")) base = zs_strndup(any, n - 8);
    else base = zs_strdup(any);
    {
        char *full = plat_full_path(base);
        paths_for_output(p, full ? full : base);
        zs_free(full);
    }
    zs_free(base);
}

void paths_free(JobPaths *p) {
    zs_free(p->output), zs_free(p->partial), zs_free(p->journal);
    memset(p, 0, sizeof(*p));
}

int paths_unfinished(const JobPaths *p) { return plat_exists(p->journal) || plat_exists(p->partial); }
