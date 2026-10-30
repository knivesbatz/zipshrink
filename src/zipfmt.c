#include "zipfmt.h"
#include "plat.h"

#define U32MAX 0xFFFFFFFFull

u64 zip_worst(u64 n) { return n + n / 1000 + 1024; }
int zip_needs_zip64(u64 n) { return zip_worst(n) >= U32MAX || n >= U32MAX; }

static void time_extras(Buf *b, const EntryMeta *m) {
    if (!m->mtime) return;
    {
        i64 secs = ft_to_unix(m->mtime);
        if (secs >= -2147483647LL - 1 && secs <= 2147483647LL) {
            buf_u16(b, EXTRA_UT);
            buf_u16(b, 5);
            buf_putc(b, 1);
            buf_u32(b, (u32)(i32)secs);
        }
    }
    buf_u16(b, EXTRA_NTFS);
    buf_u16(b, 32);
    buf_u32(b, 0);
    buf_u16(b, 1);
    buf_u16(b, 24);
    buf_u64(b, m->mtime);
    buf_u64(b, m->atime ? m->atime : m->mtime);
    buf_u64(b, m->btime ? m->btime : m->mtime);
}

static void hash_extra(Buf *b, const EntrySizes *s) {
    static const u8 zero[32];
    buf_u16(b, EXTRA_ZSHASH);
    buf_u16(b, 36);
    buf_put(b, "ZSH1", 4);
    buf_put(b, s->has_sha ? s->sha : zero, 32);
}

static u16 needed(const EntryMeta *m, const EntrySizes *s, int zip64) {
    if (zip64) return 45;
    if (m->is_dir || s->method == ZIP_DEFLATE) return 20;
    return 10;
}

void zip_local_header(Buf *out, const EntryMeta *m, const EntrySizes *s) {
    Buf ex = {0};
    u16 t, d;
    size_t nl = strlen(m->name);
    if (s->zip64_local) {
        buf_u16(&ex, EXTRA_ZIP64);
        buf_u16(&ex, 16);
        buf_u64(&ex, s->usize);
        buf_u64(&ex, s->csize);
    }
    time_extras(&ex, m);
    if (!m->is_dir) hash_extra(&ex, s);
    ft_to_dos(m->mtime, &t, &d);
    buf_u32(out, ZIP_LOCAL_SIG);
    buf_u16(out, needed(m, s, s->zip64_local));
    buf_u16(out, ZIP_FLAG_UTF8);
    buf_u16(out, s->method);
    buf_u16(out, t);
    buf_u16(out, d);
    buf_u32(out, s->crc);
    buf_u32(out, s->zip64_local ? (u32)U32MAX : (u32)s->csize);
    buf_u32(out, s->zip64_local ? (u32)U32MAX : (u32)s->usize);
    buf_u16(out, (u16)nl);
    buf_u16(out, (u16)ex.len);
    buf_put(out, m->name, nl);
    buf_put(out, ex.p, ex.len);
    buf_free(&ex);
}

void zip_central_header(Buf *out, const EntryMeta *m, const EntrySizes *s, u64 off) {
    Buf ex = {0};
    u16 t, d;
    size_t nl = strlen(m->name);
    int big = s->csize >= U32MAX || s->usize >= U32MAX || s->zip64_local;
    int bigoff = off >= U32MAX;
    if (big || bigoff) {
        buf_u16(&ex, EXTRA_ZIP64);
        buf_u16(&ex, (u16)((big ? 16 : 0) + (bigoff ? 8 : 0)));
        if (big) buf_u64(&ex, s->usize), buf_u64(&ex, s->csize);
        if (bigoff) buf_u64(&ex, off);
    }
    time_extras(&ex, m);
    if (!m->is_dir) hash_extra(&ex, s);
    ft_to_dos(m->mtime, &t, &d);
    buf_u32(out, ZIP_CENTRAL_SIG);
    buf_u16(out, (0 << 8) | 63); /* made by: MS-DOS/Windows attributes, spec 6.3 */
    buf_u16(out, needed(m, s, big || bigoff));
    buf_u16(out, ZIP_FLAG_UTF8);
    buf_u16(out, s->method);
    buf_u16(out, t);
    buf_u16(out, d);
    buf_u32(out, s->crc);
    buf_u32(out, big ? (u32)U32MAX : (u32)s->csize);
    buf_u32(out, big ? (u32)U32MAX : (u32)s->usize);
    buf_u16(out, (u16)nl);
    buf_u16(out, (u16)ex.len);
    buf_u16(out, 0);
    buf_u16(out, 0);
    buf_u16(out, 0);
    buf_u32(out, (m->attrs & 0x27) | (m->is_dir ? 0x10 : 0));
    buf_u32(out, bigoff ? (u32)U32MAX : (u32)off);
    buf_put(out, m->name, nl);
    buf_put(out, ex.p, ex.len);
    buf_free(&ex);
}

void zip_end_records(Buf *out, u64 n, u64 cd_off, u64 cd_size) {
    int z = n >= 0xFFFF || cd_off >= U32MAX || cd_size >= U32MAX;
    if (z) {
        buf_u32(out, ZIP64_EOCD_SIG);
        buf_u64(out, 44);
        buf_u16(out, 63);
        buf_u16(out, 45);
        buf_u32(out, 0);
        buf_u32(out, 0);
        buf_u64(out, n);
        buf_u64(out, n);
        buf_u64(out, cd_size);
        buf_u64(out, cd_off);
        buf_u32(out, ZIP64_LOCATOR_SIG);
        buf_u32(out, 0);
        buf_u64(out, cd_off + cd_size);
        buf_u32(out, 1);
    }
    buf_u32(out, ZIP_EOCD_SIG);
    buf_u16(out, 0);
    buf_u16(out, 0);
    buf_u16(out, z ? 0xFFFF : (u16)n);
    buf_u16(out, z ? 0xFFFF : (u16)n);
    buf_u32(out, z ? (u32)U32MAX : (u32)cd_size);
    buf_u32(out, z ? (u32)U32MAX : (u32)cd_off);
    buf_u16(out, 0);
}

u32 zip_local_len(const u8 *p) { return ZIP_LOCAL_FIXED + rd16(p + 26) + rd16(p + 28); }

static void scan_extras(const u8 *ex, size_t n, int *has_sha, u8 sha[32], u64 *mtime) {
    while (n >= 4) {
        u16 id = rd16(ex), len = rd16(ex + 2);
        if ((size_t)len + 4 > n) break;
        if (id == EXTRA_ZSHASH && len == 36 && !memcmp(ex + 4, "ZSH1", 4)) *has_sha = 1, memcpy(sha, ex + 8, 32);
        if (id == EXTRA_NTFS && len >= 32 && mtime) *mtime = rd64(ex + 12);
        ex += 4 + len, n -= 4 + (size_t)len;
    }
}

int zip_parse_local(const u8 *p, size_t n, LocalInfo *o) {
    u32 nl, el;
    const u8 *ex;
    memset(o, 0, sizeof(*o));
    if (n < ZIP_LOCAL_FIXED || rd32(p) != ZIP_LOCAL_SIG) return 0;
    nl = rd16(p + 26), el = rd16(p + 28);
    if (n < ZIP_LOCAL_FIXED + nl + el) return 0;
    o->flags = rd16(p + 6), o->method = rd16(p + 8), o->crc = rd32(p + 14);
    o->csize = rd32(p + 18), o->usize = rd32(p + 22);
    o->name = zs_strndup((const char *)p + ZIP_LOCAL_FIXED, nl);
    o->header_len = ZIP_LOCAL_FIXED + nl + el;
    ex = p + ZIP_LOCAL_FIXED + nl;
    {
        const u8 *q = ex;
        size_t left = el;
        while (left >= 4) {
            u16 id = rd16(q), len = rd16(q + 2);
            if ((size_t)len + 4 > left) break;
            if (id == EXTRA_ZIP64 && len >= 16) {
                o->zip64 = 1;
                if (o->usize == U32MAX) o->usize = rd64(q + 4);
                if (o->csize == U32MAX) o->csize = rd64(q + 12);
            }
            q += 4 + len, left -= 4 + (size_t)len;
        }
    }
    scan_extras(ex, el, &o->has_sha, o->sha, &o->mtime);
    return 1;
}

static int read_at(HANDLE h, void *buf, u32 n, u64 off) {
    u32 got = 0;
    return plat_pread(h, buf, n, off, &got) && got == n;
}

int zip_read_cd(HANDLE h, CdEntry **out, size_t *count, char **err) {
    u64 len = plat_file_len(h), tail_len, eocd_at, entries, cd_size, cd_off;
    u8 *tail, *cd;
    size_t i, pos = 0;
    long k;
    CdEntry *v;
    *out = NULL, *count = 0;
    tail_len = len < 22 + 65535 ? len : 22 + 65535;
    if (tail_len < 22) return *err = zs_strdup("the file is too small to be a ZIP archive"), 0;
    tail = zs_alloc((size_t)tail_len);
    if (!read_at(h, tail, (u32)tail_len, len - tail_len)) return zs_free(tail), *err = zs_strdup("the end of the archive cannot be read"), 0;
    for (k = (long)tail_len - 22; k >= 0; k--)
        if (rd32(tail + k) == ZIP_EOCD_SIG && (u64)k + 22 + rd16(tail + k + 20) == tail_len) break;
    if (k < 0) return zs_free(tail), *err = zs_strdup("end of central directory not found"), 0;
    eocd_at = len - tail_len + (u64)k;
    entries = rd16(tail + k + 10), cd_size = rd32(tail + k + 12), cd_off = rd32(tail + k + 16);
    zs_free(tail);
    if ((entries == 0xFFFF || cd_size == U32MAX || cd_off == U32MAX) && eocd_at >= 20) {
        u8 loc[20], rec[56];
        if (read_at(h, loc, 20, eocd_at - 20) && rd32(loc) == ZIP64_LOCATOR_SIG) {
            if (!read_at(h, rec, 56, rd64(loc + 8)) || rd32(rec) != ZIP64_EOCD_SIG) return *err = zs_strdup("bad ZIP64 end of central directory"), 0;
            entries = rd64(rec + 32), cd_size = rd64(rec + 40), cd_off = rd64(rec + 48);
        }
    }
    if (cd_off + cd_size > len || cd_size > 0x7FFFFFFF) return *err = zs_strdup("the central directory lies outside the file"), 0;
    cd = zs_alloc((size_t)cd_size + 1);
    if (cd_size && !read_at(h, cd, (u32)cd_size, cd_off)) return zs_free(cd), *err = zs_strdup("the central directory cannot be read"), 0;
    v = zs_zalloc((size_t)(entries ? entries : 1) * sizeof(CdEntry));
    for (i = 0; i < entries; i++) {
        u32 nl, el, cl;
        const u8 *ex;
        CdEntry *e = &v[i];
        if (pos + ZIP_CENTRAL_FIXED > cd_size || rd32(cd + pos) != ZIP_CENTRAL_SIG) goto bad;
        nl = rd16(cd + pos + 28), el = rd16(cd + pos + 30), cl = rd16(cd + pos + 32);
        if (pos + ZIP_CENTRAL_FIXED + nl + el + cl > cd_size) goto bad;
        e->method = rd16(cd + pos + 10), e->crc = rd32(cd + pos + 16);
        e->csize = rd32(cd + pos + 20), e->usize = rd32(cd + pos + 24);
        e->ext_attrs = rd32(cd + pos + 38), e->offset = rd32(cd + pos + 42);
        e->name = zs_strndup((const char *)cd + pos + ZIP_CENTRAL_FIXED, nl);
        ex = cd + pos + ZIP_CENTRAL_FIXED + nl;
        {
            const u8 *q = ex;
            size_t left = el;
            while (left >= 4) {
                u16 id = rd16(q), l = rd16(q + 2);
                if ((size_t)l + 4 > left) break;
                if (id == EXTRA_ZIP64) {
                    u32 o = 4;
                    if (e->usize == U32MAX) e->usize = o + 8 <= 4u + l ? rd64(q + o) : 0, o += 8;
                    if (e->csize == U32MAX) e->csize = o + 8 <= 4u + l ? rd64(q + o) : 0, o += 8;
                    if (e->offset == U32MAX) e->offset = o + 8 <= 4u + l ? rd64(q + o) : 0, o += 8;
                }
                q += 4 + l, left -= 4 + (size_t)l;
            }
        }
        scan_extras(ex, el, &e->has_sha, e->sha, NULL);
        pos += ZIP_CENTRAL_FIXED + nl + el + cl;
    }
    zs_free(cd);
    *out = v, *count = (size_t)entries;
    return 1;
bad:
    zs_free(cd);
    zip_free_cd(v, (size_t)i);
    *err = zs_strdup("corrupt central directory entry");
    return 0;
}

void zip_free_cd(CdEntry *e, size_t n) {
    size_t i;
    for (i = 0; i < n; i++) zs_free(e[i].name);
    zs_free(e);
}
