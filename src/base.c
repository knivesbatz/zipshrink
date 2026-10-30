#include <stdarg.h>
#include "base.h"

static HANDLE heap(void) {
    static HANDLE h;
    if (!h) h = GetProcessHeap();
    return h;
}

static void out_of_memory(void) {
    static const char msg[] = "ZipShrink ran out of memory. Everything committed so far is safe; resume the job later.\r\n";
    DWORD n;
    WriteFile(GetStdHandle(STD_ERROR_HANDLE), msg, sizeof(msg) - 1, &n, NULL);
    ExitProcess(70);
}

void *zs_alloc(size_t n) {
    void *p = HeapAlloc(heap(), 0, n ? n : 1);
    if (!p) out_of_memory();
    return p;
}

void *zs_zalloc(size_t n) {
    void *p = HeapAlloc(heap(), HEAP_ZERO_MEMORY, n ? n : 1);
    if (!p) out_of_memory();
    return p;
}

void *zs_realloc(void *p, size_t n) {
    void *q;
    if (!p) return zs_alloc(n);
    q = HeapReAlloc(heap(), 0, p, n ? n : 1);
    if (!q) out_of_memory();
    return q;
}

void zs_free(void *p) {
    if (p) HeapFree(heap(), 0, p);
}

/* miniz uses the C names. */
void *malloc(size_t n) { return zs_alloc(n); }
void free(void *p) { zs_free(p); }
void *realloc(void *p, size_t n) { return zs_realloc(p, n); }
void *calloc(size_t a, size_t b) { return zs_zalloc(a * b); }

void *memcpy(void *d, const void *s, size_t n) {
    u8 *dp = d;
    const u8 *sp = s;
    while (n >= 4 && (((uintptr_t)dp | (uintptr_t)sp) & 3) == 0) {
        *(u32 *)dp = *(const u32 *)sp;
        dp += 4;
        sp += 4;
        n -= 4;
    }
    while (n--) *dp++ = *sp++;
    return d;
}

void *memmove(void *d, const void *s, size_t n) {
    u8 *dp = d;
    const u8 *sp = s;
    if (dp == sp || n == 0) return d;
    if (dp < sp || dp >= sp + n) return memcpy(d, s, n);
    while (n--) dp[n] = sp[n];
    return d;
}

void *memset(void *d, int c, size_t n) {
    u8 *dp = d;
    while (n--) *dp++ = (u8)c;
    return d;
}

int memcmp(const void *a, const void *b, size_t n) {
    const u8 *x = a, *y = b;
    for (; n; n--, x++, y++)
        if (*x != *y) return *x < *y ? -1 : 1;
    return 0;
}

size_t strlen(const char *s) {
    const char *p = s;
    while (*p) p++;
    return (size_t)(p - s);
}

int zs_strcmp(const char *a, const char *b) {
    while (*a && *a == *b) a++, b++;
    return (u8)*a - (u8)*b;
}

static int lower(int c) { return c >= 'A' && c <= 'Z' ? c + 32 : c; }

int zs_stricmp(const char *a, const char *b) {
    while (*a && lower((u8)*a) == lower((u8)*b)) a++, b++;
    return lower((u8)*a) - lower((u8)*b);
}

int zs_strncmp(const char *a, const char *b, size_t n) {
    for (; n; n--, a++, b++) {
        if (*a != *b) return (u8)*a - (u8)*b;
        if (!*a) return 0;
    }
    return 0;
}

char *zs_strndup(const char *s, size_t n) {
    char *p = zs_alloc(n + 1);
    memcpy(p, s, n);
    p[n] = 0;
    return p;
}

char *zs_strdup(const char *s) { return zs_strndup(s, strlen(s)); }

int zs_starts_with(const char *s, const char *p) { return zs_strncmp(s, p, strlen(p)) == 0; }

int zs_ends_with(const char *s, const char *x) {
    size_t a = strlen(s), b = strlen(x);
    return a >= b && memcmp(s + a - b, x, b) == 0;
}

int zs_ends_with_i(const char *s, const char *x) {
    size_t a = strlen(s), b = strlen(x);
    return a >= b && zs_stricmp(s + a - b, x) == 0;
}

const char *zs_strstr(const char *h, const char *n) {
    size_t k = strlen(n);
    for (; *h; h++)
        if (zs_strncmp(h, n, k) == 0) return h;
    return k == 0 ? h : NULL;
}

char *zs_join(const char *a, const char *b) {
    size_t x = strlen(a), y = strlen(b);
    char *p = zs_alloc(x + y + 1);
    memcpy(p, a, x);
    memcpy(p + x, b, y + 1);
    return p;
}

char *zs_path_join(const char *dir, const char *name) {
    size_t n = strlen(dir);
    if (n && (dir[n - 1] == '\\' || dir[n - 1] == '/')) return zs_join(dir, name);
    {
        char *t = zs_join(dir, "\\");
        char *r = zs_join(t, name);
        zs_free(t);
        return r;
    }
}

const char *zs_basename(const char *path) {
    const char *b = path, *p;
    for (p = path; *p; p++)
        if (*p == '\\' || *p == '/' || *p == ':') b = p + 1;
    return b;
}

char *zs_dirname(const char *path) {
    const char *b = zs_basename(path);
    size_t n = (size_t)(b - path);
    while (n > 0 && (path[n - 1] == '\\' || path[n - 1] == '/')) {
        /* keep "C:\" and "\\" roots intact */
        if (n == 3 && path[1] == ':') break;
        if (n == 1) break;
        n--;
    }
    if (n == 0) return zs_strdup(".");
    return zs_strndup(path, n);
}

int zs_parse_u64(const char *s, u64 *out) {
    u64 v = 0;
    if (!*s) return 0;
    for (; *s; s++) {
        if (*s < '0' || *s > '9') return 0;
        u64 d = (u64)(*s - '0');
        if (v > (~(u64)0 - d) / 10) return 0;
        v = v * 10 + d;
    }
    *out = v;
    return 1;
}

int zs_parse_size(const char *s, u64 *out) {
    u64 whole = 0, frac = 0, fdiv = 1, mult = 1;
    const char *p = s;
    while (*p == ' ') p++;
    if (*p < '0' || *p > '9') return 0;
    while (*p >= '0' && *p <= '9') whole = whole * 10 + (u64)(*p++ - '0');
    if (*p == '.') {
        p++;
        while (*p >= '0' && *p <= '9' && fdiv < 1000000) frac = frac * 10 + (u64)(*p++ - '0'), fdiv *= 10;
    }
    while (*p == ' ') p++;
    switch (lower((u8)*p)) {
    case 0: break;
    case 'b': mult = 1; p++; break;
    case 'k': mult = 1024ULL; p++; break;
    case 'm': mult = 1024ULL * 1024; p++; break;
    case 'g': mult = 1024ULL * 1024 * 1024; p++; break;
    case 't': mult = 1024ULL * 1024 * 1024 * 1024; p++; break;
    default: return 0;
    }
    if (lower((u8)*p) == 'i') p++;
    if (lower((u8)*p) == 'b') p++;
    if (*p || whole > (~(u64)0 >> 1) / mult) return 0;
    *out = whole * mult + frac * mult / fdiv;
    return 1;
}

void buf_reserve(Buf *b, size_t extra) {
    if (b->len + extra + 1 > b->cap) {
        size_t c = b->cap ? b->cap : 64;
        while (c < b->len + extra + 1) c *= 2;
        b->p = zs_realloc(b->p, c);
        b->cap = c;
    }
}

void buf_put(Buf *b, const void *d, size_t n) {
    buf_reserve(b, n);
    memcpy(b->p + b->len, d, n);
    b->len += n;
}

void buf_puts(Buf *b, const char *s) { buf_put(b, s, strlen(s)); }
void buf_putc(Buf *b, char c) { buf_put(b, &c, 1); }

void buf_u16(Buf *b, u16 v) {
    u8 x[2] = {(u8)v, (u8)(v >> 8)};
    buf_put(b, x, 2);
}

void buf_u32(Buf *b, u32 v) {
    u8 x[4] = {(u8)v, (u8)(v >> 8), (u8)(v >> 16), (u8)(v >> 24)};
    buf_put(b, x, 4);
}

void buf_u64(Buf *b, u64 v) {
    buf_u32(b, (u32)v);
    buf_u32(b, (u32)(v >> 32));
}

char *buf_cstr(Buf *b) {
    buf_reserve(b, 0);
    b->p[b->len] = 0;
    return b->p;
}

void buf_free(Buf *b) {
    zs_free(b->p);
    b->p = NULL;
    b->len = b->cap = 0;
}

static void put_u64(Buf *b, u64 v, int base, int width, char pad) {
    char tmp[24];
    int n = 0;
    do {
        int d = (int)(v % (u64)base);
        tmp[n++] = (char)(d < 10 ? '0' + d : 'a' + d - 10);
        v /= (u64)base;
    } while (v);
    while (n < width) tmp[n++] = pad;
    while (n) buf_putc(b, tmp[--n]);
}

int zs_vfmt(Buf *b, const char *f, va_list ap) {
    for (; *f; f++) {
        int width = 0;
        char pad = ' ';
        if (*f != '%') {
            buf_putc(b, *f);
            continue;
        }
        f++;
        if (*f == '0') pad = '0', f++;
        while (*f >= '0' && *f <= '9') width = width * 10 + (*f++ - '0');
        switch (*f) {
        case 's': {
            const char *s = va_arg(ap, const char *);
            size_t n = strlen(s ? s : "(null)");
            for (; (int)n < width; width--) buf_putc(b, ' ');
            buf_puts(b, s ? s : "(null)");
            break;
        }
        case 'd': {
            int v = va_arg(ap, int);
            if (v < 0) buf_putc(b, '-'), put_u64(b, (u64)(-(i64)v), 10, width, pad);
            else put_u64(b, (u64)v, 10, width, pad);
            break;
        }
        case 'u': put_u64(b, va_arg(ap, unsigned), 10, width, pad); break;
        case 'x': put_u64(b, va_arg(ap, unsigned), 16, width, pad); break;
        case 'U': put_u64(b, va_arg(ap, u64), 10, width, pad); break;
        case 'X': put_u64(b, va_arg(ap, u64), 16, width, pad); break;
        case 'I': {
            i64 v = va_arg(ap, i64);
            if (v < 0) buf_putc(b, '-'), put_u64(b, (u64)(-v), 10, width, pad);
            else put_u64(b, (u64)v, 10, width, pad);
            break;
        }
        case 'c': buf_putc(b, (char)va_arg(ap, int)); break;
        case '%': buf_putc(b, '%'); break;
        default: buf_putc(b, '%'); buf_putc(b, *f); break;
        }
    }
    return (int)b->len;
}

void buf_fmt(Buf *b, const char *fmt, ...) {
    va_list ap;
    va_start(ap, fmt);
    zs_vfmt(b, fmt, ap);
    va_end(ap);
}

char *zs_fmt(const char *fmt, ...) {
    Buf b = {0};
    va_list ap;
    va_start(ap, fmt);
    zs_vfmt(&b, fmt, ap);
    va_end(ap);
    return buf_cstr(&b);
}

char *zs_human(u64 n) {
    static const char *units[] = {"B", "KiB", "MiB", "GiB", "TiB", "PiB"};
    u64 whole = n, scale = 1;
    int u = 0;
    if (n < 1024) return zs_fmt("%U B", n);
    while (whole >= 1024 && u < 5) whole /= 1024, scale *= 1024, u++;
    {
        /* Three significant digits, like 1.50, 15.0, 150. */
        u64 hundredths = (n % scale) * 100 / scale;
        if (whole >= 100) return zs_fmt("%U %s", whole + (hundredths >= 50), units[u]);
        if (whole >= 10) return zs_fmt("%U.%U %s", whole, hundredths / 10, units[u]);
        return zs_fmt("%U.%02U %s", whole, hundredths, units[u]);
    }
}

void pv_push(PtrVec *pv, void *p) {
    if (pv->len == pv->cap) {
        pv->cap = pv->cap ? pv->cap * 2 : 16;
        pv->v = zs_realloc(pv->v, pv->cap * sizeof(void *));
    }
    pv->v[pv->len++] = p;
}

void pv_free(PtrVec *pv, int free_items) {
    size_t i;
    if (free_items)
        for (i = 0; i < pv->len; i++) zs_free(pv->v[i]);
    zs_free(pv->v);
    pv->v = NULL;
    pv->len = pv->cap = 0;
}

WCHAR *zs_utf8_to_wide(const char *s) {
    size_t n = strlen(s), i = 0, o = 0;
    WCHAR *w = zs_alloc((n + 1) * sizeof(WCHAR));
    const u8 *p = (const u8 *)s;
    while (i < n) {
        u32 c = p[i];
        int extra = c >= 0xF0 ? 3 : c >= 0xE0 ? 2 : c >= 0xC0 ? 1 : 0;
        if (c >= 0x80 && c < 0xC0) extra = -1;
        if (extra < 0 || i + (size_t)extra >= n + (extra ? 0 : 1)) {
            w[o++] = 0xFFFD;
            i++;
            continue;
        }
        if (extra) {
            int k;
            c &= 0x3F >> extra;
            for (k = 1; k <= extra; k++) c = (c << 6) | (p[i + k] & 0x3F);
        }
        i += (size_t)extra + 1;
        if (c >= 0x10000) {
            c -= 0x10000;
            w[o++] = (WCHAR)(0xD800 + (c >> 10));
            w[o++] = (WCHAR)(0xDC00 + (c & 0x3FF));
        } else {
            w[o++] = (WCHAR)c;
        }
    }
    w[o] = 0;
    return w;
}

char *zs_wide_to_utf8(const WCHAR *w) {
    Buf b = {0};
    for (; *w; w++) {
        u32 c = *w;
        if (c >= 0xD800 && c < 0xDC00 && w[1] >= 0xDC00 && w[1] < 0xE000) {
            c = 0x10000 + ((c - 0xD800) << 10) + (w[1] - 0xDC00);
            w++;
        } else if (c >= 0xD800 && c < 0xE000) {
            c = 0xFFFD; /* unpaired surrogate */
        }
        if (c < 0x80) buf_putc(&b, (char)c);
        else if (c < 0x800) buf_putc(&b, (char)(0xC0 | (c >> 6))), buf_putc(&b, (char)(0x80 | (c & 0x3F)));
        else if (c < 0x10000)
            buf_putc(&b, (char)(0xE0 | (c >> 12))), buf_putc(&b, (char)(0x80 | ((c >> 6) & 0x3F))), buf_putc(&b, (char)(0x80 | (c & 0x3F)));
        else
            buf_putc(&b, (char)(0xF0 | (c >> 18))), buf_putc(&b, (char)(0x80 | ((c >> 12) & 0x3F))),
                buf_putc(&b, (char)(0x80 | ((c >> 6) & 0x3F))), buf_putc(&b, (char)(0x80 | (c & 0x3F)));
    }
    return buf_cstr(&b);
}

int zs_utf8_valid(const char *s) {
    WCHAR *w = zs_utf8_to_wide(s);
    int ok = 1;
    WCHAR *p;
    for (p = w; *p; p++)
        if (*p == 0xFFFD) ok = 0;
    zs_free(w);
    return ok;
}

char *zs_acp_to_utf8(const char *s) {
    int n = MultiByteToWideChar(CP_ACP, 0, s, -1, NULL, 0);
    WCHAR *w;
    char *r;
    if (n <= 0) return zs_strdup(s);
    w = zs_alloc((size_t)n * sizeof(WCHAR));
    MultiByteToWideChar(CP_ACP, 0, s, -1, w, n);
    r = zs_wide_to_utf8(w);
    zs_free(w);
    return r;
}

char *zs_utf8_to_acp(const char *s, int *lossy) {
    WCHAR *w = zs_utf8_to_wide(s);
    BOOL used = FALSE;
    int n = WideCharToMultiByte(CP_ACP, 0, w, -1, NULL, 0, NULL, NULL);
    char *a = zs_alloc(n > 0 ? (size_t)n : 1);
    if (n > 0) WideCharToMultiByte(CP_ACP, 0, w, -1, a, n, "?", &used);
    else a[0] = 0;
    if (lossy) *lossy = used ? 1 : 0;
    zs_free(w);
    return a;
}

void *zs_proc(const char *dll, const char *name) {
    HMODULE m = GetModuleHandleA(dll);
    if (!m) m = LoadLibraryA(dll);
    return m ? (void *)GetProcAddress(m, name) : NULL;
}

size_t zs_wcslen(const WCHAR *w) {
    const WCHAR *p = w;
    while (*p) p++;
    return (size_t)(p - w);
}

int zs_is_nt(void) {
    static int v = -1;
    if (v < 0) v = (GetVersion() & 0x80000000u) ? 0 : 1;
    return v;
}

const char *zs_os_name(void) {
    typedef LONG(WINAPI * RGV)(OSVERSIONINFOW *);
    static char name[48];
    OSVERSIONINFOA vi;
    memset(&vi, 0, sizeof(vi));
    vi.dwOSVersionInfoSize = sizeof(vi);
    if (!GetVersionExA(&vi)) return "Windows";
    if (vi.dwPlatformId == VER_PLATFORM_WIN32_WINDOWS) {
        if (vi.dwMinorVersion == 0) return "Windows 95";
        if (vi.dwMinorVersion < 90) return "Windows 98";
        return "Windows Me";
    }
    {
        /* GetVersionEx reports 6.2 to programs not declared for Windows 8.1
         * and later; RtlGetVersion tells the truth. */
        RGV rgv = (RGV)zs_proc("ntdll.dll", "RtlGetVersion");
        OSVERSIONINFOW w;
        memset(&w, 0, sizeof(w));
        w.dwOSVersionInfoSize = sizeof(w);
        if (rgv && rgv(&w) == 0) vi.dwMajorVersion = w.dwMajorVersion, vi.dwMinorVersion = w.dwMinorVersion, vi.dwBuildNumber = w.dwBuildNumber;
    }
    if (vi.dwMajorVersion <= 3) return "Windows NT 3.51";
    if (vi.dwMajorVersion == 4) return "Windows NT 4.0";
    if (vi.dwMajorVersion == 5) return vi.dwMinorVersion == 0 ? "Windows 2000" : vi.dwMinorVersion == 1 ? "Windows XP" : "Windows Server 2003";
    if (vi.dwMajorVersion == 6) {
        static const char *six[] = {"Windows Vista", "Windows 7", "Windows 8", "Windows 8.1"};
        return vi.dwMinorVersion < 4 ? six[vi.dwMinorVersion] : "Windows";
    }
    {
        Buf b = {0};
        buf_fmt(&b, "Windows %s (build %u)", (vi.dwBuildNumber & 0xFFFF) >= 22000 ? "11" : "10", (unsigned)(vi.dwBuildNumber & 0xFFFF));
        if (b.len < sizeof(name)) memcpy(name, b.p, b.len), name[b.len] = 0;
        buf_free(&b);
        return name;
    }
}

static void write_console(DWORD which, const char *s) {
    HANDLE h = GetStdHandle(which);
    DWORD n, mode;
    if (!h || h == INVALID_HANDLE_VALUE) return;
    if (zs_is_nt() && GetConsoleMode(h, &mode)) {
        typedef BOOL(WINAPI * WCW)(HANDLE, const void *, DWORD, DWORD *, void *);
        WCW wcw = (WCW)zs_proc("kernel32.dll", "WriteConsoleW");
        WCHAR *w = zs_utf8_to_wide(s);
        if (wcw) wcw(h, w, (DWORD)zs_wcslen(w), &n, NULL);
        zs_free(w);
    } else if (GetConsoleMode(h, &mode)) {
        /* Windows 9x console: OEM code page. */
        WCHAR *w = zs_utf8_to_wide(s);
        int k = WideCharToMultiByte(CP_OEMCP, 0, w, -1, NULL, 0, NULL, NULL);
        char *a = zs_alloc(k > 0 ? (size_t)k : 1);
        if (k > 0) WideCharToMultiByte(CP_OEMCP, 0, w, -1, a, k, "?", NULL);
        WriteFile(h, a, k > 0 ? (DWORD)(k - 1) : 0, &n, NULL);
        zs_free(a);
        zs_free(w);
    } else {
        WriteFile(h, s, (DWORD)strlen(s), &n, NULL); /* redirected: UTF-8 */
    }
}

void zs_out(const char *s) { write_console(STD_OUTPUT_HANDLE, s); }
void zs_err(const char *s) { write_console(STD_ERROR_HANDLE, s); }

void zs_outf(const char *fmt, ...) {
    Buf b = {0};
    va_list ap;
    va_start(ap, fmt);
    zs_vfmt(&b, fmt, ap);
    va_end(ap);
    zs_out(buf_cstr(&b));
    buf_free(&b);
}

void zs_errf(const char *fmt, ...) {
    Buf b = {0};
    va_list ap;
    va_start(ap, fmt);
    zs_vfmt(&b, fmt, ap);
    va_end(ap);
    zs_err(buf_cstr(&b));
    buf_free(&b);
}

/* Splits a command line by the Microsoft C rules (quotes and backslashes). */
static char **split_args(const char *cmd, int *argc) {
    PtrVec v = {0};
    const char *p = cmd;
    for (;;) {
        Buf a = {0};
        int quoted = 0, any = 0;
        while (*p == ' ' || *p == '\t') p++;
        if (!*p) break;
        while (*p && (quoted || (*p != ' ' && *p != '\t'))) {
            if (*p == '\\') {
                size_t bs = 0;
                while (*p == '\\') bs++, p++;
                if (*p == '"') {
                    size_t i;
                    for (i = 0; i < bs / 2; i++) buf_putc(&a, '\\');
                    if (bs % 2) buf_putc(&a, '"'), p++;
                } else {
                    size_t i;
                    for (i = 0; i < bs; i++) buf_putc(&a, '\\');
                }
            } else if (*p == '"') {
                if (quoted && p[1] == '"') buf_putc(&a, '"'), p++;
                else quoted = !quoted;
                p++;
            } else {
                buf_putc(&a, *p++);
            }
            any = 1;
        }
        (void)any;
        pv_push(&v, buf_cstr(&a));
    }
    *argc = (int)v.len;
    pv_push(&v, NULL);
    return (char **)v.v;
}

char **zs_args(int *argc) {
    typedef WCHAR *(WINAPI * GCLW)(void);
    GCLW gclw = zs_is_nt() ? (GCLW)zs_proc("kernel32.dll", "GetCommandLineW") : NULL;
    if (gclw) {
        char *u = zs_wide_to_utf8(gclw());
        char **r = split_args(u, argc);
        zs_free(u);
        return r;
    } else {
        char *u = zs_acp_to_utf8(GetCommandLineA());
        char **r = split_args(u, argc);
        zs_free(u);
        return r;
    }
}

char *zs_getenv(const char *name) {
    char buf[1024];
    DWORD n = GetEnvironmentVariableA(name, buf, sizeof(buf));
    if (n == 0 || n >= sizeof(buf)) return NULL;
    return zs_acp_to_utf8(buf);
}

u32 zs_ticks(void) { return GetTickCount(); }

i64 zs_unix_now(void) {
    FILETIME ft;
    u64 t;
    GetSystemTimeAsFileTime(&ft);
    t = ((u64)ft.dwHighDateTime << 32) | ft.dwLowDateTime;
    return (i64)(t / 10000000ULL) - 11644473600LL;
}
