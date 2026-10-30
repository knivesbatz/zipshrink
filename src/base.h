/* ZipShrink runtime: the few C library pieces the program needs, built on
 * KERNEL32 alone so the executables load on every 32-bit Windows from
 * Windows 95 onwards (the C runtimes of current compilers import functions
 * Windows 95 does not have). */
#ifndef ZS_BASE_H
#define ZS_BASE_H

#define WIN32_LEAN_AND_MEAN
#ifndef _WIN32_WINNT
#define _WIN32_WINNT 0x0400
#endif
#ifndef WINVER
#define WINVER 0x0400
#endif
#include <windows.h>
#include <stddef.h>
#include <stdint.h>

typedef unsigned long long u64;
typedef long long i64;
typedef unsigned int u32;
typedef int i32;
typedef unsigned short u16;
typedef unsigned char u8;

#include "version.h"
#define ARRAY_LEN(a) (sizeof(a) / sizeof((a)[0]))

/* Memory (process heap). Allocation failure ends the process with a message:
 * every caller would otherwise have to unwind, and nothing is half-done on
 * disk at an allocation point that the journal cannot recover. */
void *zs_alloc(size_t n);
void *zs_zalloc(size_t n);
void *zs_realloc(void *p, size_t n);
void zs_free(void *p);

void *memcpy(void *d, const void *s, size_t n);
void *memmove(void *d, const void *s, size_t n);
void *memset(void *d, int c, size_t n);
int memcmp(const void *a, const void *b, size_t n);
size_t strlen(const char *s);

int zs_strcmp(const char *a, const char *b);
int zs_stricmp(const char *a, const char *b); /* ASCII case-insensitive */
int zs_strncmp(const char *a, const char *b, size_t n);
char *zs_strdup(const char *s);
char *zs_strndup(const char *s, size_t n);
int zs_starts_with(const char *s, const char *prefix);
int zs_ends_with(const char *s, const char *suffix);
int zs_ends_with_i(const char *s, const char *suffix);
const char *zs_strstr(const char *h, const char *n);
char *zs_join(const char *a, const char *b); /* a + b */
char *zs_path_join(const char *dir, const char *name);
const char *zs_basename(const char *path);
char *zs_dirname(const char *path);
int zs_parse_u64(const char *s, u64 *out);
int zs_parse_size(const char *s, u64 *out); /* "512M", "2G" */

/* Growable byte buffer. */
typedef struct {
    char *p;
    size_t len, cap;
} Buf;
void buf_reserve(Buf *b, size_t extra);
void buf_put(Buf *b, const void *data, size_t n);
void buf_puts(Buf *b, const char *s);
void buf_putc(Buf *b, char c);
void buf_u16(Buf *b, u16 v);
void buf_u32(Buf *b, u32 v);
void buf_u64(Buf *b, u64 v);
void buf_fmt(Buf *b, const char *fmt, ...);
char *buf_cstr(Buf *b); /* NUL-terminates and returns b->p */
void buf_free(Buf *b);

/* printf subset: %s %d %u %x %08x %c %%, and %U %I (u64, i64), %X (u64 hex). */
int zs_vfmt(Buf *out, const char *fmt, va_list ap);
char *zs_fmt(const char *fmt, ...); /* heap string */
char *zs_human(u64 bytes);          /* "1.50 GiB" (heap string) */

/* Growable array of pointers. */
typedef struct {
    void **v;
    size_t len, cap;
} PtrVec;
void pv_push(PtrVec *pv, void *p);
void pv_free(PtrVec *pv, int free_items);

/* UTF-8 <-> UTF-16 (own codecs: CP_UTF8 is missing on Windows 95). */
WCHAR *zs_utf8_to_wide(const char *s); /* heap */
char *zs_wide_to_utf8(const WCHAR *w);  /* heap */
char *zs_acp_to_utf8(const char *s);    /* system code page -> UTF-8 */
char *zs_utf8_to_acp(const char *s, int *lossy);
int zs_utf8_valid(const char *s);

/* Little-endian readers. */
static inline u16 rd16(const u8 *p) { return (u16)(p[0] | (p[1] << 8)); }
static inline u32 rd32(const u8 *p) { return (u32)p[0] | ((u32)p[1] << 8) | ((u32)p[2] << 16) | ((u32)p[3] << 24); }
static inline u64 rd64(const u8 *p) { return (u64)rd32(p) | ((u64)rd32(p + 4) << 32); }

/* Looks up a function at run time (NULL if absent). Every wide-character
 * and post-1995 API goes through this, so the import table stays within
 * what Windows 95 provides. */
void *zs_proc(const char *dll, const char *name);
size_t zs_wcslen(const WCHAR *w);

/* Platform version. */
int zs_is_nt(void);          /* Windows NT family (NT 3.51 .. 11) */
const char *zs_os_name(void); /* "Windows 95", "Windows 10", ... */

/* Output for console programs (UTF-8 text converted for the console). */
void zs_out(const char *s);
void zs_err(const char *s);
void zs_outf(const char *fmt, ...);
void zs_errf(const char *fmt, ...);

/* Command line as UTF-8 arguments (argv[0] is the program). */
char **zs_args(int *argc);

/* Environment variable as UTF-8, or NULL. */
char *zs_getenv(const char *name);

/* Monotonic milliseconds. */
u32 zs_ticks(void);
/* Seconds since 1970 (UTC). */
i64 zs_unix_now(void);

#endif
