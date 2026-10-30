#include "verify.h"

void inflater_init(Inflater *f, u16 method) {
    memset(f, 0, sizeof(*f));
    f->method = method;
    sha256_init(&f->sha);
    if (method == ZIP_DEFLATE) {
        f->z = zs_alloc(sizeof(tinfl_decompressor));
        tinfl_init(f->z);
        f->dict = zs_alloc(TINFL_LZ_DICT_SIZE);
    } else {
        f->ended = 1;
    }
}

static void emit(Inflater *f, const u8 *p, size_t n) {
    f->crc = crc32_update(f->crc, p, n);
    sha256_update(&f->sha, p, n);
    f->out += n;
}

static int run(Inflater *f, const u8 *data, size_t n, int more) {
    while (!f->ended) {
        size_t in_n = n, out_n = TINFL_LZ_DICT_SIZE - f->dict_ofs;
        tinfl_status st = tinfl_decompress(f->z, data, &in_n, f->dict, f->dict + f->dict_ofs, &out_n, more ? TINFL_FLAG_HAS_MORE_INPUT : 0);
        data += in_n, n -= in_n;
        if (out_n) emit(f, f->dict + f->dict_ofs, out_n);
        f->dict_ofs = (f->dict_ofs + out_n) & (TINFL_LZ_DICT_SIZE - 1);
        if (st == TINFL_STATUS_DONE) {
            f->ended = 1;
            if (n) f->trailing = 1;
            return 1;
        }
        if (st < 0) {
            f->corrupt = 1;
            return 0;
        }
        if (st == TINFL_STATUS_NEEDS_MORE_INPUT) {
            if (n == 0) return more ? 1 : (f->corrupt = 1, 0);
            continue;
        }
        /* HAS_MORE_OUTPUT: loop to drain the dictionary window. */
    }
    if (n) f->trailing = 1;
    return 1;
}

int inflater_feed(Inflater *f, const u8 *data, size_t n) {
    f->in += n;
    if (f->method == ZIP_STORE) {
        emit(f, data, n);
        return 1;
    }
    if (f->corrupt) return 0;
    if (f->ended) {
        if (n) f->trailing = 1;
        return 1;
    }
    return run(f, data, n, 1);
}

void inflater_finish(Inflater *f, u32 *crc, u8 sha[32], u64 *out, int *ok) {
    if (f->method == ZIP_DEFLATE && !f->ended && !f->corrupt) run(f, NULL, 0, 0);
    *crc = f->crc;
    sha256_final(&f->sha, sha);
    *out = f->out;
    *ok = f->ended && !f->trailing && !f->corrupt;
}

void inflater_free(Inflater *f) {
    zs_free(f->z);
    zs_free(f->dict);
    f->z = NULL, f->dict = NULL;
}

typedef struct {
    Buf head;
    int have_header;
    u32 hlen;
    Inflater inf;
    char *why;
} EntryCtx;

static int entry_sink(void *c, const u8 *data, u32 n) {
    EntryCtx *e = c;
    if (e->why) return 1;
    if (!e->have_header) {
        buf_put(&e->head, data, n);
        if (e->head.len < ZIP_LOCAL_FIXED) return 1;
        e->hlen = zip_local_len((const u8 *)e->head.p);
        if (e->head.len < e->hlen) return 1;
        e->have_header = 1;
        if (e->head.len > e->hlen && !inflater_feed(&e->inf, (const u8 *)e->head.p + e->hlen, e->head.len - e->hlen)) e->why = zs_strdup("the compressed data is corrupt");
        return 1;
    }
    if (!inflater_feed(&e->inf, data, n)) e->why = zs_strdup("the compressed data is corrupt");
    return 1;
}

CheckResult check_entry(HANDLE h, const char *path, const Expected *x, Readback *mode, char **why) {
    EntryCtx e;
    LocalInfo li;
    u32 crc;
    u8 sha[32];
    u64 out;
    int ok;
    CheckResult res = CHK_OK;
    memset(&e, 0, sizeof(e));
    inflater_init(&e.inf, x->method);
    *why = NULL;
    if (!plat_read_uncached(path, h, x->offset, x->end - x->offset, entry_sink, &e, mode)) {
        *why = zs_fmt("it could not be read back: %s", plat_error(GetLastError()));
        res = CHK_IO;
        goto done;
    }
    if (e.why) {
        *why = e.why, e.why = NULL;
        res = CHK_MISMATCH;
        goto done;
    }
    if (!e.have_header || !zip_parse_local((const u8 *)e.head.p, e.hlen, &li)) {
        *why = zs_strdup("its local header is unreadable");
        res = CHK_MISMATCH;
        goto done;
    }
    if (x->header_crc && crc32_update(0, e.head.p, e.hlen) != x->header_crc) *why = zs_strdup("its header bytes differ from what was written");
    else if (zs_strcmp(li.name, x->name)) *why = zs_strdup("its header names a different file");
    else if (li.method != x->method || li.crc != x->crc || li.csize != x->csize || li.usize != x->usize || li.zip64 != x->zip64_local)
        *why = zs_strdup("its header sizes or checksum differ from what was written");
    else if (x->sha && (!li.has_sha || memcmp(li.sha, x->sha, 32))) *why = zs_strdup("its stored SHA-256 differs from what was written");
    else if (e.hlen + x->csize != x->end - x->offset) *why = zs_strdup("its length does not match its header");
    zs_free(li.name);
    if (*why) {
        res = CHK_MISMATCH;
        goto done;
    }
    inflater_finish(&e.inf, &crc, sha, &out, &ok);
    if (!ok) *why = zs_strdup("its compressed data is incomplete or has trailing bytes");
    else if (out != x->usize || crc != x->crc) *why = zs_strdup("its data does not match the source (CRC-32)");
    else if (x->sha && memcmp(sha, x->sha, 32)) *why = zs_strdup("its data does not match the source (SHA-256)");
    if (*why) res = CHK_MISMATCH;
done:
    inflater_free(&e.inf);
    buf_free(&e.head);
    return res;
}

int archive_ok(const ArchiveCheck *c) { return c->problems.len == 0; }
void archive_check_free(ArchiveCheck *c) {
    pv_free(&c->problems, 1);
    memset(c, 0, sizeof(*c));
}

/* Whole-archive check: walks the central directory, re-reads every local
 * header, and (with `data`) inflates every entry, checking CRC-32 and the
 * stored SHA-256. */
int check_archive(const char *path, int data, volatile LONG *cancel, BytesCb cb, void *ctx, ArchiveCheck *out, char **err) {
    HANDLE h = plat_open_read(path, 0);
    CdEntry *cd;
    size_t n, i;
    u8 *buf;
    memset(out, 0, sizeof(*out));
    out->data_checked = data;
    if (h == INVALID_HANDLE_VALUE) return *err = zs_fmt("%s: %s", path, plat_error(GetLastError())), 0;
    if (!zip_read_cd(h, &cd, &n, err)) return CloseHandle(h), 0;
    buf = zs_alloc(1 << 16);
    for (i = 0; i < n; i++) {
        CdEntry *c = &cd[i];
        u8 hdr[ZIP_LOCAL_FIXED];
        u8 *full;
        u32 got = 0, hlen;
        LocalInfo li;
        int is_dir = strlen(c->name) && c->name[strlen(c->name) - 1] == '/';
        if (cancel && *cancel) {
            zs_free(buf), zip_free_cd(cd, n), CloseHandle(h);
            return *err = zs_strdup("stopped at your request"), 0;
        }
        out->entries++;
        if (!plat_pread(h, hdr, ZIP_LOCAL_FIXED, c->offset, &got) || got != ZIP_LOCAL_FIXED || rd32(hdr) != ZIP_LOCAL_SIG) {
            pv_push(&out->problems, zs_fmt("%s: its local header is missing", c->name));
            continue;
        }
        hlen = zip_local_len(hdr);
        full = zs_alloc(hlen);
        if (!plat_pread(h, full, hlen, c->offset, &got) || got != hlen || !zip_parse_local(full, hlen, &li)) {
            zs_free(full);
            pv_push(&out->problems, zs_fmt("%s: its local header is unreadable", c->name));
            continue;
        }
        zs_free(full);
        if (zs_strcmp(li.name, c->name) || li.crc != c->crc || li.csize != c->csize || li.usize != c->usize || li.method != c->method) {
            zs_free(li.name);
            pv_push(&out->problems, zs_fmt("%s: the local header and central directory disagree", c->name));
            continue;
        }
        zs_free(li.name);
        if (is_dir) {
            out->dirs++;
            continue;
        }
        out->files++;
        out->bytes += c->usize;
        if (!data) continue;
        {
            Inflater f;
            u64 pos = c->offset + hlen, end = pos + c->csize;
            u32 crc;
            u8 sha[32];
            u64 total;
            int ok, ioerr = 0;
            inflater_init(&f, c->method);
            if (c->method != ZIP_STORE && c->method != ZIP_DEFLATE) {
                pv_push(&out->problems, zs_fmt("%s: unsupported compression method %u", c->name, c->method));
                inflater_free(&f);
                continue;
            }
            while (pos < end) {
                u32 want = end - pos < (1 << 16) ? (u32)(end - pos) : (1 << 16);
                if (!plat_pread(h, buf, want, pos, &got) || got != want) {
                    ioerr = 1;
                    break;
                }
                inflater_feed(&f, buf, got);
                pos += got;
                if (cb) cb(ctx, got);
                if (cancel && *cancel) break;
            }
            inflater_finish(&f, &crc, sha, &total, &ok);
            inflater_free(&f);
            if (ioerr) pv_push(&out->problems, zs_fmt("%s: could not be read: %s", c->name, plat_error(GetLastError())));
            else if (!ok || total != c->usize) pv_push(&out->problems, zs_fmt("%s: the compressed data is corrupt", c->name));
            else if (crc != c->crc) pv_push(&out->problems, zs_fmt("%s: CRC-32 does not match", c->name));
            else if (c->has_sha && memcmp(sha, c->sha, 32)) pv_push(&out->problems, zs_fmt("%s: SHA-256 does not match", c->name));
            else if (c->has_sha) out->sha_checked++;
            else out->crc_only++;
        }
    }
    zs_free(buf);
    zip_free_cd(cd, n);
    CloseHandle(h);
    return 1;
}

/* ---- salvage ---- */

typedef struct {
    u64 offset;
    char *name;
    int is_dir;
    EntrySizes sz;
    u64 mtime;
} Found;

/* A valid entry at `off`? Fills f and *end. */
static int entry_at(HANDLE h, u64 off, u64 len, Found *f, u64 *end) {
    u8 fixed[ZIP_LOCAL_FIXED], *raw, *buf;
    u32 got = 0, hlen;
    LocalInfo li;
    Inflater inf;
    u64 pos, stop;
    u32 crc;
    u8 sha[32];
    u64 out;
    int ok;
    if (off + ZIP_LOCAL_FIXED > len) return 0;
    if (!plat_pread(h, fixed, ZIP_LOCAL_FIXED, off, &got) || got != ZIP_LOCAL_FIXED || rd32(fixed) != ZIP_LOCAL_SIG) return 0;
    hlen = zip_local_len(fixed);
    if (off + hlen > len) return 0;
    raw = zs_alloc(hlen);
    if (!plat_pread(h, raw, hlen, off, &got) || got != hlen || !zip_parse_local(raw, hlen, &li)) return zs_free(raw), 0;
    zs_free(raw);
    if ((li.flags & 8) || (li.method != ZIP_STORE && li.method != ZIP_DEFLATE) || !li.name[0] || !zs_utf8_valid(li.name)) return zs_free(li.name), 0;
    stop = off + hlen + li.csize;
    if (stop > len || stop < off) return zs_free(li.name), 0;
    if (li.name[strlen(li.name) - 1] == '/' && (li.csize || li.usize)) return zs_free(li.name), 0;
    inflater_init(&inf, li.method);
    buf = zs_alloc(1 << 16);
    for (pos = off + hlen; pos < stop;) {
        u32 want = stop - pos < (1 << 16) ? (u32)(stop - pos) : (1 << 16);
        if (!plat_pread(h, buf, want, pos, &got) || got != want || !inflater_feed(&inf, buf, got)) break;
        pos += got;
    }
    zs_free(buf);
    inflater_finish(&inf, &crc, sha, &out, &ok);
    inflater_free(&inf);
    if (pos < stop || !ok || out != li.usize || crc != li.crc || (li.has_sha && memcmp(sha, li.sha, 32))) return zs_free(li.name), 0;
    memset(f, 0, sizeof(*f));
    f->offset = off;
    f->name = li.name;
    f->is_dir = li.name[strlen(li.name) - 1] == '/';
    f->mtime = li.mtime;
    f->sz.method = li.method, f->sz.crc = li.crc, f->sz.csize = li.csize, f->sz.usize = li.usize;
    f->sz.has_sha = li.has_sha, f->sz.zip64_local = li.zip64;
    memcpy(f->sz.sha, li.sha, 32);
    *end = stop;
    return 1;
}

static int next_sig(HANDLE h, u64 from, u64 len, u64 *at) {
    u8 *buf = zs_alloc(1 << 16);
    u64 pos = from;
    while (pos + 4 <= len) {
        u32 want = len - pos < (1 << 16) ? (u32)(len - pos) : (1 << 16), got = 0, i;
        if (!plat_pread(h, buf, want, pos, &got) || got < 4) break;
        for (i = 0; i + 4 <= got; i++)
            if (rd32(buf + i) == ZIP_LOCAL_SIG) {
                *at = pos + i;
                zs_free(buf);
                return 1;
            }
        pos += got - 3;
    }
    zs_free(buf);
    return 0;
}

char *salvage_default_output(const char *partial) {
    char *base = zs_ends_with_i(partial, ".partial") ? zs_strndup(partial, strlen(partial) - 8) : zs_join(partial, ".zip");
    int k;
    if (!plat_exists(base)) return base;
    for (k = 1;; k++) {
        char *stem = zs_ends_with_i(base, ".zip") ? zs_strndup(base, strlen(base) - 4) : zs_strdup(base);
        char *c = zs_fmt("%s (recovered %d).zip", stem, k);
        zs_free(stem);
        if (!plat_exists(c)) return zs_free(base), c;
        zs_free(c);
    }
}

int salvage(const char *partial, const char *output, volatile LONG *cancel, SalvageReport *rep, char **err, ErrKind *kind) {
    HANDLE h;
    u64 len, pos = 0, last_end = 0;
    Found *found = NULL;
    size_t nf = 0, i;
    Buf cd = {0};
    memset(rep, 0, sizeof(*rep));
    *kind = ERR_IO;
    if (plat_exists(output)) return *kind = ERR_EXISTS, *err = zs_fmt("%s already exists", output), 0;
    h = plat_open_rw(partial);
    if (h == INVALID_HANDLE_VALUE) return *err = zs_fmt("%s: %s", partial, plat_error(GetLastError())), 0;
    len = plat_file_len(h);
    while (pos < len) {
        Found f;
        u64 end;
        if (cancel && *cancel) {
            CloseHandle(h);
            return *err = zs_strdup("stopped at your request"), 0;
        }
        if (entry_at(h, pos, len, &f, &end)) {
            found = zs_realloc(found, (nf + 1) * sizeof(Found));
            found[nf++] = f;
            pos = last_end = end;
        } else {
            u64 probe = pos + 1, s;
            int resumed = 0;
            rep->damaged_ranges++;
            while (next_sig(h, probe, len, &s)) {
                Found g;
                u64 e2;
                if (entry_at(h, s, len, &g, &e2)) {
                    zs_free(g.name);
                    pos = s;
                    resumed = 1;
                    break;
                }
                probe = s + 1;
            }
            if (!resumed) break;
        }
    }
    if (!nf) {
        CloseHandle(h);
        *kind = ERR_DAMAGED;
        return *err = zs_fmt("%s holds no complete, valid entries", partial), 0;
    }
    /* A damaged tail (an unfinished entry or old central directory) is
     * replaced; damage between valid entries stays where it is. */
    for (i = 0; i < nf; i++) {
        EntryMeta m;
        memset(&m, 0, sizeof(m));
        m.name = found[i].name, m.is_dir = found[i].is_dir, m.mtime = found[i].mtime;
        m.attrs = found[i].is_dir ? 0x10 : 0;
        zip_central_header(&cd, &m, &found[i].sz, found[i].offset);
    }
    {
        u64 cd_size = cd.len;
        zip_end_records(&cd, nf, last_end, cd_size);
    }
    if (!plat_set_len(h, last_end) || !plat_pwrite(h, cd.p, (u32)cd.len, last_end) || !plat_flush(h)) {
        *err = zs_fmt("%s: %s", partial, plat_error(GetLastError()));
        CloseHandle(h);
        goto fail;
    }
    CloseHandle(h);
    buf_free(&cd);
    if (!check_archive(partial, 1, cancel, NULL, NULL, &rep->check, err)) goto fail;
    if (!archive_ok(&rep->check)) {
        *kind = ERR_DAMAGED;
        *err = zs_fmt("the rebuilt archive failed verification: %s", (char *)rep->check.problems.v[0]);
        goto fail;
    }
    if (!plat_rename_noreplace(partial, output)) {
        *err = zs_fmt("%s: %s", output, plat_error(GetLastError()));
        goto fail;
    }
    rep->output = zs_strdup(output);
    rep->entries = nf;
    for (i = 0; i < nf; i++) {
        if (found[i].is_dir) rep->dirs++;
        else rep->files++, rep->bytes += found[i].sz.usize;
        zs_free(found[i].name);
    }
    zs_free(found);
    *kind = ERR_NONE;
    return 1;
fail:
    for (i = 0; i < nf; i++) zs_free(found[i].name);
    zs_free(found);
    buf_free(&cd);
    return 0;
}
