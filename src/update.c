#include "update.h"
#include "hash.h"
#include "plat.h"

typedef void *HINET;
typedef HINET(WINAPI *OpenA)(const char *, DWORD, const char *, const char *, DWORD);
typedef HINET(WINAPI *OpenUrlA)(HINET, const char *, const char *, DWORD, DWORD, DWORD_PTR);
typedef BOOL(WINAPI *ReadF)(HINET, void *, DWORD, DWORD *);
typedef BOOL(WINAPI *CloseH)(HINET);
typedef BOOL(WINAPI *QueryA)(HINET, DWORD, void *, DWORD *, DWORD *);

#define I_OPEN_PRECONFIG 0
#define I_FLAG_RELOAD 0x80000000u
#define I_FLAG_NO_CACHE_WRITE 0x04000000u
#define I_FLAG_NO_UI 0x00000200u
#define I_FLAG_NO_COOKIES 0x00080000u
#define Q_STATUS 19
#define Q_LENGTH 5
#define Q_NUMBER 0x20000000u

static struct {
    int loaded;
    HMODULE dll;
    OpenA open;
    OpenUrlA open_url;
    ReadF read;
    CloseH close;
    QueryA query;
} net;

static int load_wininet(char **err) {
    if (!net.loaded) {
        net.loaded = 1;
        net.dll = LoadLibraryA("wininet.dll");
        if (net.dll) {
            net.open = (OpenA)GetProcAddress(net.dll, "InternetOpenA");
            net.open_url = (OpenUrlA)GetProcAddress(net.dll, "InternetOpenUrlA");
            net.read = (ReadF)GetProcAddress(net.dll, "InternetReadFile");
            net.close = (CloseH)GetProcAddress(net.dll, "InternetCloseHandle");
            net.query = (QueryA)GetProcAddress(net.dll, "HttpQueryInfoA");
        }
    }
    if (!net.open || !net.open_url || !net.read || !net.close || !net.query) {
        *err = zs_strdup("this copy of Windows has no Internet support (WININET.DLL, which comes with Internet Explorer 3 or later). "
                         "Check for updates at https://github.com/" UPDATE_REPO "/releases");
        return 0;
    }
    return 1;
}

static char *net_error(DWORD e, const char *url) {
    /* 12157: secure channel error; 12175: security error; 12029/12030: cannot connect. */
    if (e == 12157 || e == 12175 || (!zs_is_nt() && (e == 12029 || e == 12030)))
        return zs_fmt("could not make a secure connection to GitHub (error %u). Windows 95, 98, Me and older versions of NT cannot use the encryption "
                      "GitHub requires; download updates from https://github.com/" UPDATE_REPO "/releases on another computer.",
                      (unsigned)e);
    if (e == 12007) return zs_strdup("could not find github.com: check the Internet connection");
    if (e == 12029 || e == 12002) return zs_fmt("could not connect to %s: check the Internet connection (error %u)", url, (unsigned)e);
    return zs_fmt("could not download %s (error %u)", url, (unsigned)e);
}

/* GET url; calls each(chunk) and returns 1 on HTTP 200. */
typedef int (*Each)(void *ctx, const u8 *d, u32 n, u64 total);
static int http_get(const char *url, const char *headers, Each each, void *ctx, char **err) {
    HINET s, h;
    DWORD status = 0, len = sizeof(status), total = 0, tl = sizeof(total), idx = 0;
    u8 *buf;
    int ok = 1;
    if (!load_wininet(err)) return 0;
    s = net.open("ZipShrink/" ZS_VERSION, I_OPEN_PRECONFIG, NULL, NULL, 0);
    if (!s) return *err = net_error(GetLastError(), url), 0;
    h = net.open_url(s, url, headers, headers ? (DWORD)-1 : 0, I_FLAG_RELOAD | I_FLAG_NO_CACHE_WRITE | I_FLAG_NO_UI | I_FLAG_NO_COOKIES, 0);
    if (!h) {
        *err = net_error(GetLastError(), url);
        net.close(s);
        return 0;
    }
    if (!net.query(h, Q_STATUS | Q_NUMBER, &status, &len, &idx) || status != 200) {
        *err = status == 404               ? zs_strdup("no release has been published yet")
               : status == 403 || status == 429 ? zs_strdup("GitHub is limiting requests from this network right now; try again in an hour")
                                                : zs_fmt("GitHub answered %u for %s", (unsigned)status, url);
        net.close(h), net.close(s);
        return 0;
    }
    idx = 0;
    if (!net.query(h, Q_LENGTH | Q_NUMBER, &total, &tl, &idx)) total = 0;
    buf = zs_alloc(64 << 10);
    for (;;) {
        DWORD got = 0;
        if (!net.read(h, buf, 64 << 10, &got)) {
            *err = net_error(GetLastError(), url);
            ok = 0;
            break;
        }
        if (!got) break;
        if (!each(ctx, buf, got, total)) {
            ok = 0;
            break;
        }
    }
    zs_free(buf);
    net.close(h), net.close(s);
    return ok;
}

typedef struct {
    Buf b;
    size_t limit;
    char **err;
} Collect;

static int collect(void *ctx, const u8 *d, u32 n, u64 total) {
    Collect *c = ctx;
    (void)total;
    if (c->b.len + n > c->limit) return *c->err = zs_strdup("the answer from GitHub was unexpectedly large"), 0;
    buf_put(&c->b, d, n);
    return 1;
}

void release_free(Release *r) {
    zs_free(r->version), zs_free(r->tag), zs_free(r->page), zs_free(r->installer_url), zs_free(r->sums_url);
    memset(r, 0, sizeof(*r));
}

/* Reads the JSON string starting at the opening quote *p. */
static char *json_string(const char *p) {
    Buf b = {0};
    if (*p != '"') return NULL;
    for (p++; *p && *p != '"'; p++) {
        if (*p == '\\' && p[1]) {
            p++;
            buf_putc(&b, *p == 'n' ? '\n' : *p == 't' ? '\t' : *p == 'u' ? '?' : *p);
            if (*p == 'u') {
                int k;
                for (k = 0; k < 4 && p[1]; k++) p++;
            }
        } else {
            buf_putc(&b, *p);
        }
    }
    return buf_cstr(&b);
}

/* The string value of the next "key" at or after `from`. */
static char *json_value(const char *from, const char *key, const char **after) {
    char *pat = zs_fmt("\"%s\"", key);
    const char *p = zs_strstr(from, pat);
    char *v = NULL;
    if (p) {
        p += strlen(pat);
        while (*p == ' ' || *p == '\n' || *p == '\r' || *p == '\t') p++;
        if (*p == ':') {
            p++;
            while (*p == ' ' || *p == '\n' || *p == '\r' || *p == '\t') p++;
            v = json_string(p);
            if (after) *after = p + 1;
        }
    }
    zs_free(pat);
    return v;
}

static void parse_version(const char *s, u32 v[3]) {
    int i = 0;
    v[0] = v[1] = v[2] = 0;
    if (*s == 'v' || *s == 'V') s++;
    for (; *s && i < 3; s++) {
        if (*s >= '0' && *s <= '9') v[i] = v[i] * 10 + (u32)(*s - '0');
        else if (*s == '.') i++;
        else break;
    }
}

int version_newer(const char *a, const char *b) {
    u32 x[3], y[3];
    int i;
    parse_version(a, x);
    parse_version(b, y);
    for (i = 0; i < 3; i++)
        if (x[i] != y[i]) return x[i] > y[i];
    return 0;
}

int update_check(Release *r, int *newer, char **err) {
    Collect c;
    const char *p;
    char *url;
    memset(r, 0, sizeof(*r));
    memset(&c, 0, sizeof(c));
    c.limit = 4 << 20, c.err = err;
    *newer = 0;
    if (!http_get("https://api.github.com/repos/" UPDATE_REPO "/releases/latest", "Accept: application/vnd.github+json\r\n", collect, &c, err)) return buf_free(&c.b), 0;
    buf_cstr(&c.b);
    r->tag = json_value(c.b.p, "tag_name", NULL);
    if (!r->tag) {
        buf_free(&c.b);
        return *err = zs_strdup("GitHub's answer did not name a release"), 0;
    }
    r->version = zs_strdup(r->tag[0] == 'v' ? r->tag + 1 : r->tag);
    r->page = zs_fmt("https://github.com/" UPDATE_REPO "/releases/tag/%s", r->tag);
    p = c.b.p;
    while ((url = json_value(p, "browser_download_url", &p)) != NULL) {
        const char *base = url;
        const char *s;
        for (s = url; *s; s++)
            if (*s == '/') base = s + 1;
        if (!zs_strcmp(base, UPDATE_INSTALLER) && !r->installer_url) r->installer_url = url;
        else if (!zs_strcmp(base, UPDATE_CHECKSUMS) && !r->sums_url) r->sums_url = url;
        else zs_free(url);
    }
    buf_free(&c.b);
    *newer = version_newer(r->version, ZS_VERSION);
    return 1;
}

static char *exe_dir(void) {
    char buf[MAX_PATH + 1];
    DWORD n = GetModuleFileNameA(NULL, buf, MAX_PATH);
    char *u, *d;
    if (!n || n >= MAX_PATH) return NULL;
    buf[n] = 0;
    u = zs_acp_to_utf8(buf);
    d = zs_dirname(u);
    zs_free(u);
    return d;
}

int update_can_install(void) {
    char *d = exe_dir(), *m;
    int portable;
    if (!d) return 0;
    m = zs_path_join(d, UPDATE_PORTABLE_MARKER);
    portable = plat_exists(m);
    zs_free(m), zs_free(d);
    return !portable;
}

static int hex_digit(char c) { return c >= '0' && c <= '9' ? c - '0' : c >= 'a' && c <= 'f' ? c - 'a' + 10 : c >= 'A' && c <= 'F' ? c - 'A' + 10 : -1; }

int update_expected_hash(const char *sums, const char *name, u8 out[32]) {
    const char *line = sums;
    size_t nl = strlen(name);
    while (*line) {
        const char *e = line, *f;
        int i, ok = 1;
        while (*e && *e != '\n') e++;
        for (i = 0; i < 64 && line + i < e; i++)
            if (hex_digit(line[i]) < 0) ok = 0;
        if (ok && e - line > 66) {
            f = line + 64;
            while (f < e && (*f == ' ' || *f == '\t')) f++;
            if (f < e && *f == '*') f++;
            {
                size_t fl = (size_t)(e - f);
                while (fl && (f[fl - 1] == '\r' || f[fl - 1] == ' ')) fl--;
                if (fl == nl && !zs_strncmp(f, name, nl)) {
                    for (i = 0; i < 32; i++) out[i] = (u8)(hex_digit(line[2 * i]) << 4 | hex_digit(line[2 * i + 1]));
                    return 1;
                }
            }
        }
        line = *e ? e + 1 : e;
    }
    return 0;
}

typedef struct {
    HANDLE h;
    u64 done;
    Sha256 sha;
    DownloadCb cb;
    void *ctx;
    char **err;
} Save;

static int save(void *ctx, const u8 *d, u32 n, u64 total) {
    Save *s = ctx;
    if (s->done + n > (u64)1 << 30) return *s->err = zs_strdup("the installer is unexpectedly large"), 0;
    if (!plat_pwrite(s->h, d, n, s->done)) return *s->err = zs_fmt("cannot save the installer: %s", plat_error(GetLastError())), 0;
    sha256_update(&s->sha, d, n);
    s->done += n;
    if (s->cb) s->cb(s->ctx, s->done, total);
    return 1;
}

int update_download(const Release *r, char **path, DownloadCb cb, void *ctx, char **err) {
    Collect c;
    Save s;
    u8 want[32], got[32];
    char tmp[MAX_PATH + 1], *dir, *name, *full;
    DWORD n;
    *path = NULL;
    if (!r->installer_url || !r->sums_url) return *err = zs_fmt("release %s has no %s or %s", r->tag, UPDATE_INSTALLER, UPDATE_CHECKSUMS), 0;
    memset(&c, 0, sizeof(c));
    c.limit = 1 << 20, c.err = err;
    if (!http_get(r->sums_url, NULL, collect, &c, err)) return buf_free(&c.b), 0;
    if (!update_expected_hash(buf_cstr(&c.b), UPDATE_INSTALLER, want)) {
        buf_free(&c.b);
        return *err = zs_fmt("%s does not list %s", UPDATE_CHECKSUMS, UPDATE_INSTALLER), 0;
    }
    buf_free(&c.b);
    n = GetTempPathA(MAX_PATH, tmp);
    if (!n || n >= MAX_PATH) return *err = zs_strdup("cannot find the temporary folder"), 0;
    tmp[n] = 0;
    dir = zs_acp_to_utf8(tmp);
    name = zs_fmt("ZipShrink-%s-setup.exe", r->version);
    full = zs_path_join(dir, name);
    zs_free(dir), zs_free(name);
    plat_delete_file(full);
    memset(&s, 0, sizeof(s));
    s.h = plat_create_new(full);
    if (s.h == INVALID_HANDLE_VALUE) {
        *err = zs_fmt("cannot create %s: %s", full, plat_error(GetLastError()));
        zs_free(full);
        return 0;
    }
    sha256_init(&s.sha);
    s.cb = cb, s.ctx = ctx, s.err = err;
    if (!http_get(r->installer_url, NULL, save, &s, err) || !plat_flush(s.h)) {
        if (!*err) *err = zs_fmt("cannot save the installer: %s", plat_error(GetLastError()));
        CloseHandle(s.h);
        plat_delete_file(full);
        zs_free(full);
        return 0;
    }
    CloseHandle(s.h);
    sha256_final(&s.sha, got);
    if (memcmp(got, want, 32)) {
        plat_delete_file(full);
        zs_free(full);
        return *err = zs_strdup("the downloaded installer does not match the published SHA-256; it was deleted and nothing was installed"), 0;
    }
    *path = full;
    return 1;
}

int update_launch(const char *path, char **err) {
    STARTUPINFOA si;
    PROCESS_INFORMATION pi;
    int lossy = 0;
    char *a = zs_utf8_to_acp(path, &lossy), *cmd;
    BOOL ok;
    if (lossy) return zs_free(a), *err = zs_fmt("cannot start %s: its name cannot be represented on this system", path), 0;
    cmd = zs_fmt("\"%s\"", a);
    memset(&si, 0, sizeof(si));
    si.cb = sizeof(si);
    ok = CreateProcessA(a, cmd, NULL, NULL, FALSE, 0, NULL, NULL, &si, &pi);
    zs_free(a), zs_free(cmd);
    if (!ok) return *err = zs_fmt("cannot start the installer: %s", plat_error(GetLastError())), 0;
    CloseHandle(pi.hThread);
    CloseHandle(pi.hProcess);
    return 1;
}

void update_open_url(const char *url) {
    typedef HINSTANCE(WINAPI * SEA)(HWND, const char *, const char *, const char *, const char *, int);
    HMODULE sh = LoadLibraryA("shell32.dll");
    SEA sea = sh ? (SEA)GetProcAddress(sh, "ShellExecuteA") : NULL;
    if (sea) sea(NULL, "open", url, NULL, NULL, SW_SHOWNORMAL);
}
