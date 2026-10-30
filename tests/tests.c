/* In-process tests of the runtime, formats and engine (zs-tests.exe).
 * tests/run.py runs it along with the end-to-end tests. */
#include "engine.h"
#include "failpoint.h"
#include "hash.h"
#include "journal.h"
#include "update.h"
#include "verify.h"
#include "miniz.h"

static int failures, checks;

#define CHECK(c)                                                                                                                                      \
    do {                                                                                                                                              \
        checks++;                                                                                                                                     \
        if (!(c)) failures++, zs_errf("FAIL %s:%d: %s\n", __FILE__, __LINE__, #c);                                                                    \
    } while (0)

static void test_hashes(void) {
    u8 d[32];
    char hex[65];
    Sha256 s;
    CHECK(crc32_update(0, "123456789", 9) == 0xCBF43926u);
    CHECK(crc32_update(crc32_update(0, "1234", 4), "56789", 5) == 0xCBF43926u);
    sha256_init(&s);
    sha256_update(&s, "abc", 3);
    sha256_final(&s, d);
    hex_encode(d, 32, hex);
    CHECK(!zs_strcmp(hex, "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"));
    sha256_init(&s);
    sha256_final(&s, d);
    hex_encode(d, 32, hex);
    CHECK(!zs_strcmp(hex, "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"));
    {
        /* 1,000,000 x 'a' fed in odd pieces. */
        static u8 buf[1001];
        int i;
        memset(buf, 'a', sizeof(buf));
        sha256_init(&s);
        for (i = 0; i < 1000; i++) sha256_update(&s, buf, i % 2 ? 999 : 1001);
        sha256_final(&s, d);
        hex_encode(d, 32, hex);
        CHECK(!zs_strcmp(hex, "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"));
    }
    CHECK(mz_adler32(1, (const u8 *)"Wikipedia", 9) == 0x11E60398u);
}

static void test_strings(void) {
    u64 v;
    char *h;
    CHECK(zs_parse_size("512M", &v) && v == 512ull << 20);
    CHECK(zs_parse_size("2G", &v) && v == 2ull << 30);
    CHECK(zs_parse_size("1.5G", &v) && v == 3ull << 29);
    CHECK(zs_parse_size("100", &v) && v == 100);
    CHECK(!zs_parse_size("lots", &v));
    h = zs_human(1536);
    CHECK(!zs_strcmp(h, "1.50 KiB"));
    zs_free(h);
    h = zs_human(1023);
    CHECK(!zs_strcmp(h, "1023 B"));
    zs_free(h);
    h = zs_fmt("%s|%5s|%03d|%U|%X|%I", "a", "b", 7, 12345678901234ull, 0xABCDEFull, -42ll);
    CHECK(!zs_strcmp(h, "a|    b|007|12345678901234|abcdef|-42"));
    zs_free(h);
    {
        const char *s = "caf\xc3\xa9 \xe6\x97\xa5 \xf0\x9f\x98\x80";
        WCHAR *w = zs_utf8_to_wide(s);
        char *back = zs_wide_to_utf8(w);
        CHECK(zs_wcslen(w) == 9);
        CHECK(!zs_strcmp(back, s));
        CHECK(zs_utf8_valid(s));
        CHECK(!zs_utf8_valid("\xc3"));
        zs_free(w), zs_free(back);
    }
    CHECK(path_within("C:\\a\\b\\c.zip", "C:\\a"));
    CHECK(path_within("C:\\A\\b", "c:\\a"));
    CHECK(!path_within("C:\\ab\\c", "C:\\a"));
    CHECK(version_newer("0.10.0", "0.9.9"));
    CHECK(version_newer("v1.0.0", "0.2.0"));
    CHECK(!version_newer("0.2.0", "0.2.0"));
    CHECK(!version_newer("0.1.9", "0.2.0"));
}

static void test_sums(void) {
    u8 h[32];
    char sums[400];
    const char *hx = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
    char *s = zs_fmt("%s  other.zip\n%s *ZipShrink-setup.exe\r\n", "cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd", hx);
    memcpy(sums, s, strlen(s) + 1);
    zs_free(s);
    CHECK(update_expected_hash(sums, "ZipShrink-setup.exe", h) && h[1] == 0x11 && h[31] == 0xff);
    CHECK(!update_expected_hash(sums, "ZipShrink-setup", h));
    CHECK(update_expected_hash(sums, "other.zip", h) && h[0] == 0xcd);
}

static void test_journal(void) {
    Journal j;
    Loaded l;
    Buf b = {0};
    char *e = NULL;
    int busy;
    const char *path = "zs-test.journal";
    plat_delete_file(path);
    CHECK(journal_create(&j, path));
    rec_begin(&b, 'H');
    rec_str(&b, "format", "zipshrink-journal-c1");
    rec_str(&b, "odd", "tab\there\nnew\\line\r");
    journal_add(&j, &b);
    rec_begin(&b, 'C');
    rec_num(&b, "i", 18446744073709551615ull);
    journal_add(&j, &b);
    CHECK(journal_sync(&j));
    {
        /* A torn final record (no newline) is ignored. */
        const char torn[] = "deadbeef C\ti=1";
        CHECK(plat_pwrite(j.h, torn, sizeof(torn) - 1, j.len));
    }
    journal_close(&j);
    CHECK(journal_load(path, &l, &e, &busy));
    CHECK(l.count == 2 && l.torn_tail);
    CHECK(l.count == 2 && !zs_strcmp(rec_get(&l.recs[0], "odd"), "tab\there\nnew\\line\r"));
    CHECK(l.count == 2 && rec_u64(&l.recs[1], "i") == 18446744073709551615ull);
    journal_free_loaded(&l);
    {
        /* Damage before the end stops loading. */
        HANDLE h = plat_open_rw(path);
        char c = 'X';
        CHECK(h != INVALID_HANDLE_VALUE && plat_pwrite(h, &c, 1, 12));
        CloseHandle(h);
        CHECK(!journal_load(path, &l, &e, &busy));
        zs_free(e);
    }
    buf_free(&b);
    plat_delete_file(path);
}

static void test_inflate(void) {
    static u8 src[300000];
    u32 i, x = 12345;
    tdefl_compressor *c = zs_alloc(sizeof(tdefl_compressor));
    size_t cap = sizeof(src) + 1024, in = sizeof(src), outn = cap;
    u8 *out = zs_alloc(cap), sha[32], want[32];
    Inflater f;
    u32 crc;
    u64 n;
    int ok;
    Sha256 s;
    for (i = 0; i < sizeof(src); i++) x = x * 1103515245 + 12345, src[i] = (u8)(i % 1000 < 500 ? 'a' + i % 7 : x >> 16);
    tdefl_init(c, NULL, NULL, (int)tdefl_create_comp_flags_from_zip_params(6, -15, 0));
    CHECK(tdefl_compress(c, src, &in, out, &outn, TDEFL_FINISH) == TDEFL_STATUS_DONE);
    inflater_init(&f, 8);
    for (i = 0; i < outn; i += 777) CHECK(inflater_feed(&f, out + i, outn - i < 777 ? outn - i : 777));
    inflater_finish(&f, &crc, sha, &n, &ok);
    sha256_init(&s);
    sha256_update(&s, src, sizeof(src));
    sha256_final(&s, want);
    CHECK(ok && n == sizeof(src) && crc == crc32_update(0, src, sizeof(src)) && !memcmp(sha, want, 32));
    inflater_free(&f);
    /* Corrupt stream. */
    out[outn / 2] ^= 0x55;
    inflater_init(&f, 8);
    inflater_feed(&f, out, outn);
    inflater_finish(&f, &crc, sha, &n, &ok);
    CHECK(!ok || crc != crc32_update(0, src, sizeof(src)));
    inflater_free(&f);
    zs_free(out), zs_free(c);
}

static void write_file(const char *path, const char *data, u32 n) {
    HANDLE h;
    plat_delete_file(path);
    h = plat_create_new(path);
    CHECK(h != INVALID_HANDLE_VALUE);
    CHECK(plat_pwrite(h, data, n, 0));
    CloseHandle(h);
}

static void test_split(void) {
    Scan s;
    Part *parts;
    char *e = NULL;
    PlanEntry pe;
    int i, k;
    u64 total = 0;
    memset(&s, 0, sizeof(s));
    for (i = 0; i < 30; i++) {
        memset(&pe, 0, sizeof(pe));
        pe.idx = (u64)i;
        pe.src = zs_fmt("C:\\x\\f%d", i);
        pe.name = zs_fmt("x/f%d", i);
        pe.size = (u64)(1000 + i * 37);
        scan_add(&s, &pe);
        s.files++, s.bytes += pe.size;
        total += pe.size;
        zs_free(pe.src), zs_free(pe.name);
    }
    CHECK(split_scan(&s, "C:\\out.zip", 4, 500, &parts, &e));
    {
        u64 sum = 0, mx = 0, mn = (u64)-1;
        for (k = 0; k < 4; k++) {
            sum += parts[k].bytes;
            if (parts[k].bytes > mx) mx = parts[k].bytes;
            if (parts[k].bytes < mn) mn = parts[k].bytes;
        }
        CHECK(sum == total);
        CHECK(mx - mn < 3000); /* within about two files */
        CHECK(!zs_strcmp(parts[0].output, "C:\\out.part1.zip") && !zs_strcmp(parts[3].output, "C:\\out.part4.zip"));
    }
    parts_free(parts, 4);
    CHECK(!split_scan(&s, "C:\\out.zip", 31, 500, &parts, &e));
    zs_free(e);
    /* By size (files of 40-84 KB): every part's estimate stays within the
     * size, except a part holding one file that is larger on its own. */
    for (i = 0; i < 30; i++) s.e[i].size *= 40;
    total *= 40;
    {
        int n = 0, alone = 0;
        u64 sum = 0;
        CHECK(split_scan_size(&s, "C:\\out.zip", 200000, 1000, &parts, &n, &e));
        CHECK(n >= 10 && n <= 15);
        for (k = 0; k < n; k++) {
            CHECK(parts[k].estimated <= 200000);
            CHECK(parts[k].files > 0);
            sum += parts[k].bytes;
        }
        CHECK(sum == total);
        parts_free(parts, n);
        CHECK(split_scan_size(&s, "C:\\out.zip", 70000, 1000, &parts, &n, &e));
        CHECK(n == 30);
        for (k = 0; k < n; k++)
            if (parts[k].estimated > 70000) {
                alone++;
                CHECK(parts[k].files == 1);
            }
        CHECK(alone > 0);
        parts_free(parts, n);
        /* Everything fits in one: no split. */
        CHECK(split_scan_size(&s, "C:\\out.zip", 100000000, 1000, &parts, &n, &e));
        CHECK(n == 1 && parts == NULL);
        CHECK(!split_scan_size(&s, "C:\\out.zip", 1000, 1000, &parts, &n, &e));
        zs_free(e);
    }
    scan_free(&s);
    {
        char *p = part_path("C:\\a\\b.zip", 7, 12);
        CHECK(!zs_strcmp(p, "C:\\a\\b.part07.zip"));
        zs_free(p);
    }
}

/* The engine keeps an original that changes after it was read, even when
 * its size and modification time are put back (the SHA-256 re-check). */
static const char *g_victim;
static void change_victim(void *ctx) {
    HANDLE h = plat_open_rw(g_victim);
    FILETIME c, a, w;
    (void)ctx;
    if (h != INVALID_HANDLE_VALUE) {
        GetFileTime(h, &c, &a, &w);
        plat_pwrite(h, "ORIGINAL", 8, 0);
        SetFileTime(h, &c, &a, &w);
        CloseHandle(h);
    }
    zs_fp_hook("before_delete", NULL, NULL);
}

static void test_changed_original_kept(void) {
    char *root, *a, *b, *out, *e = NULL, **roots;
    size_t nroots;
    Scan s;
    Options o;
    Job *j;
    ErrKind k;
    Report *r;
    const char *srcs[1];
    CreateDirectoryA("zs-chg", NULL);
    root = plat_full_path("zs-chg");
    a = zs_path_join(root, "a.txt");
    b = zs_path_join(root, "b.txt");
    write_file(a, "original a\n", 11);
    write_file(b, "original b\n", 11);
    out = plat_full_path("zs-chg.zip");
    plat_delete_file(out);
    srcs[0] = root;
    CHECK(scan_roots(srcs, 1, &roots, &nroots, &e));
    memset(&s, 0, sizeof(s));
    CHECK(scan_build(&s, roots, nroots, NULL, 0, NULL, &e));
    options_default(&o);
    o.delete_originals = 1;
    o.min_free = MIN_FREE_FLOOR;
    j = job_create(&o, roots, nroots, &s, out, &e, &k);
    CHECK(j != NULL);
    if (j) {
        g_victim = a;
        zs_fp_hook("before_delete", change_victim, NULL);
        r = job_run(j, NULL, NULL);
        if (r->outcome != OUT_ISSUES) zs_errf("outcome %d deleted %U kept %U: %s\n", (int)r->outcome, r->deleted, (u64)r->nkept, r->error ? r->error : "");
        CHECK(r->outcome == OUT_ISSUES);
        CHECK(r->deleted == 1 && r->nkept == 1);
        CHECK(plat_exists(a) && !plat_exists(b));
        report_free(r);
        job_free(j);
    }
    plat_delete_file(a), plat_delete_file(out);
    plat_remove_dir(root);
    scan_free(&s);
    {
        size_t i;
        for (i = 0; i < nroots; i++) zs_free(roots[i]);
        zs_free(roots);
    }
    zs_free(root), zs_free(a), zs_free(b), zs_free(out);
}

void zs_test_start(void) {
    test_hashes();
    test_strings();
    test_sums();
    test_journal();
    test_inflate();
    test_split();
    test_changed_original_kept();
    zs_outf("%d checks, %d failed (%s)\n", checks, failures, zs_os_name());
    ExitProcess(failures ? 1 : 0);
}
