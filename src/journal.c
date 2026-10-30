#include "journal.h"
#include "hash.h"
#include "plat.h"
#include "failpoint.h"

const char *rec_get(const Rec *r, const char *key) {
    int i;
    for (i = 0; i < r->n; i++)
        if (!zs_strcmp(r->key[i], key)) return r->val[i];
    return NULL;
}

int rec_has(const Rec *r, const char *key) { return rec_get(r, key) != NULL; }

u64 rec_u64(const Rec *r, const char *key) {
    const char *v = rec_get(r, key);
    u64 x = 0;
    if (v) zs_parse_u64(v, &x);
    return x;
}

void rec_begin(Buf *b, char type) {
    b->len = 0;
    buf_puts(b, "00000000 ");
    buf_putc(b, type);
}

void rec_str(Buf *b, const char *key, const char *val) {
    buf_putc(b, '\t');
    buf_puts(b, key);
    buf_putc(b, '=');
    for (; *val; val++) {
        switch (*val) {
        case '\\': buf_puts(b, "\\\\"); break;
        case '\t': buf_puts(b, "\\t"); break;
        case '\n': buf_puts(b, "\\n"); break;
        case '\r': buf_puts(b, "\\r"); break;
        default: buf_putc(b, *val);
        }
    }
}

void rec_num(Buf *b, const char *key, u64 v) {
    char *s = zs_fmt("%U", v);
    rec_str(b, key, s);
    zs_free(s);
}

static void seal(Buf *b) {
    char hex[9];
    u32 c = crc32_update(0, b->p + 9, b->len - 9);
    int i;
    for (i = 0; i < 8; i++) hex[i] = "0123456789abcdef"[(c >> (28 - 4 * i)) & 15];
    memcpy(b->p, hex, 8);
    buf_putc(b, '\n');
}

int journal_create(Journal *j, const char *path) {
    memset(j, 0, sizeof(*j));
    j->h = plat_create_new(path);
    if (j->h == INVALID_HANDLE_VALUE) return 0;
    plat_lock(j->h);
    j->path = zs_strdup(path);
    return 1;
}

int journal_reopen(Journal *j, const char *path, u64 valid_len, int *busy) {
    memset(j, 0, sizeof(*j));
    *busy = 0;
    j->h = plat_open_rw(path);
    if (j->h == INVALID_HANDLE_VALUE) {
        *busy = plat_is_sharing(GetLastError());
        return 0;
    }
    if (!plat_lock(j->h)) {
        CloseHandle(j->h);
        *busy = 1;
        return 0;
    }
    if (plat_file_len(j->h) != valid_len && (!plat_set_len(j->h, valid_len) || !plat_flush(j->h))) {
        CloseHandle(j->h);
        return 0;
    }
    j->path = zs_strdup(path);
    j->len = valid_len;
    return 1;
}

void journal_add(Journal *j, Buf *rec) {
    seal(rec);
    buf_put(&j->pending, rec->p, rec->len);
    rec->len = 0;
}

int journal_sync(Journal *j) {
    if (j->pending.len) {
        if (zs_fail_torn()) {
            /* Test aid: half a record reaches the disk, then a "crash". */
            plat_pwrite(j->h, j->pending.p, (u32)(j->pending.len / 2 + 1), j->len);
            plat_flush(j->h);
            zs_err("failpoint journal_torn: aborting\n");
            TerminateProcess(GetCurrentProcess(), ZS_FP_EXIT);
        }
        if (!plat_pwrite(j->h, j->pending.p, (u32)j->pending.len, j->len)) return 0;
        j->len += j->pending.len;
        j->pending.len = 0;
    }
    return plat_flush(j->h);
}

void journal_close(Journal *j) {
    if (j->h && j->h != INVALID_HANDLE_VALUE) {
        UnlockFile(j->h, 0, 0, 1, 0);
        CloseHandle(j->h);
    }
    j->h = INVALID_HANDLE_VALUE;
    zs_free(j->path);
    j->path = NULL;
    buf_free(&j->pending);
}

void rec_free(Rec *r) {
    int i;
    for (i = 0; i < r->n; i++) zs_free(r->key[i]), zs_free(r->val[i]);
    zs_free(r->key);
    zs_free(r->val);
}

static int parse_line(const char *line, size_t n, Rec *r) {
    u32 want = 0, got;
    size_t i;
    memset(r, 0, sizeof(*r));
    if (n < 10 || line[8] != ' ') return 0;
    for (i = 0; i < 8; i++) {
        char c = line[i];
        int v = c >= '0' && c <= '9' ? c - '0' : c >= 'a' && c <= 'f' ? c - 'a' + 10 : -1;
        if (v < 0) return 0;
        want = want << 4 | (u32)v;
    }
    got = crc32_update(0, line + 9, n - 9);
    if (got != want) return 0;
    r->type = line[9];
    i = 10;
    while (i < n) {
        Buf k = {0}, v = {0};
        if (line[i] != '\t') return rec_free(r), 0;
        i++;
        while (i < n && line[i] != '=' && line[i] != '\t') buf_putc(&k, line[i++]);
        if (i >= n || line[i] != '=') return buf_free(&k), rec_free(r), 0;
        i++;
        while (i < n && line[i] != '\t') {
            if (line[i] == '\\' && i + 1 < n) {
                char c = line[i + 1];
                buf_putc(&v, c == 't' ? '\t' : c == 'n' ? '\n' : c == 'r' ? '\r' : c);
                i += 2;
            } else {
                buf_putc(&v, line[i++]);
            }
        }
        r->key = zs_realloc(r->key, (size_t)(r->n + 1) * sizeof(char *));
        r->val = zs_realloc(r->val, (size_t)(r->n + 1) * sizeof(char *));
        r->key[r->n] = buf_cstr(&k);
        r->val[r->n] = buf_cstr(&v);
        r->n++;
    }
    return 1;
}

int journal_load(const char *path, Loaded *out, char **err, int *busy) {
    HANDLE h = plat_open_read(path, 0);
    u64 len, pos = 0;
    char *data;
    u32 got = 0;
    size_t cap = 0;
    memset(out, 0, sizeof(*out));
    *busy = 0;
    if (h == INVALID_HANDLE_VALUE) {
        DWORD e = GetLastError();
        *busy = plat_is_sharing(e);
        *err = zs_fmt("cannot read the journal: %s", plat_error(e));
        return 0;
    }
    len = plat_file_len(h);
    if (len > 0x7FFFFFFF) return CloseHandle(h), *err = zs_strdup("the journal is too large"), 0;
    data = zs_alloc((size_t)len + 1);
    if (len && (!plat_pread(h, data, (u32)len, 0, &got) || got != len)) {
        DWORD e = GetLastError();
        CloseHandle(h);
        zs_free(data);
        *busy = plat_is_sharing(e);
        *err = zs_fmt("cannot read the journal: %s", plat_error(e));
        return 0;
    }
    CloseHandle(h);
    while (pos < len) {
        u64 nl = pos;
        Rec r;
        while (nl < len && data[nl] != '\n') nl++;
        if (nl >= len) {
            out->torn_tail = 1; /* unterminated last line */
            break;
        }
        if (!parse_line(data + pos, (size_t)(nl - pos), &r)) {
            if (nl + 1 >= len) {
                out->torn_tail = 1;
                break;
            }
            *err = zs_fmt("the journal is damaged at byte %U", pos);
            zs_free(data);
            journal_free_loaded(out);
            return 0;
        }
        if (out->count == cap) cap = cap ? cap * 2 : 256, out->recs = zs_realloc(out->recs, cap * sizeof(Rec));
        out->recs[out->count++] = r;
        pos = nl + 1;
        if (out->count == 1) out->first_len = pos;
    }
    zs_free(data);
    out->valid_len = pos;
    if (out->count && (out->recs[0].type != 'H' || !rec_get(&out->recs[0], "format") || zs_strcmp(rec_get(&out->recs[0], "format"), "zipshrink-journal-c1"))) {
        journal_free_loaded(out);
        *err = zs_strdup("this is not a ZipShrink journal, or it was written by another version");
        return 0;
    }
    return 1;
}

void journal_free_loaded(Loaded *l) {
    size_t i;
    for (i = 0; i < l->count; i++) rec_free(&l->recs[i]);
    zs_free(l->recs);
    memset(l, 0, sizeof(*l));
}
