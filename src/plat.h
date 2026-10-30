/* File system access with UTF-8 paths, for Windows 95 through 11.
 *
 * On the NT family the wide-character API is used (looked up at run time),
 * so any Unicode name works. On Windows 95/98/Me the ANSI API is used; a
 * name the system code page cannot represent is reported, never guessed.
 */
#ifndef ZS_PLAT_H
#define ZS_PLAT_H
#include "base.h"

/* Identity and metadata of a file at one moment. Times are FILETIMEs. */
typedef struct {
    u64 size;
    u64 mtime, atime, btime;
    u32 volume;
    u64 index; /* NTFS file id; meaningless on FAT, compared only on NT */
    u32 attrs;
} FStat;

int fstat_same(const FStat *a, const FStat *b);
const char *fstat_difference(const FStat *was, const FStat *now);

typedef enum { K_FILE, K_DIR, K_LINK, K_OTHER, K_MISSING } Kind;

typedef struct {
    char *name; /* UTF-8, NULL when the entry cannot be named here */
    u32 attrs;
    u64 size, mtime, atime, btime;
} DirEntry;

/* Error text for the last failure (UTF-8, static buffer). */
const char *plat_error(DWORD code);
int plat_is_disk_full(DWORD code);
int plat_is_sharing(DWORD code);

int plat_kind(const char *path, Kind *kind, FStat *st); /* 0 on error (GetLastError) */
char *plat_full_path(const char *path);                 /* NULL on error */
int plat_exists(const char *path);

/* Lists a directory; returns 0 on error. Entries are owned by the caller. */
int plat_list_dir(const char *dir, DirEntry **entries, size_t *count);
void plat_free_entries(DirEntry *e, size_t n);

HANDLE plat_open_read(const char *path, int deny_writers); /* INVALID_HANDLE_VALUE on error */
HANDLE plat_create_new(const char *path);
HANDLE plat_open_rw(const char *path);
int plat_stat_handle(HANDLE h, FStat *st);
int plat_pwrite(HANDLE h, const void *buf, u32 n, u64 off);
int plat_pread(HANDLE h, void *buf, u32 n, u64 off, u32 *got);
int plat_set_len(HANDLE h, u64 len);
u64 plat_file_len(HANDLE h);
int plat_flush(HANDLE h);
int plat_lock(HANDLE h); /* exclusive lock on byte 0; 0 if another process holds it */

typedef enum { RB_UNCACHED, RB_CACHED } Readback;
/* Reads [off, off+len) through a handle that bypasses the file cache where the
 * system allows it (NT); calls sink for each piece. Returns 0 on I/O error. */
typedef int (*Sink)(void *ctx, const u8 *data, u32 n);
int plat_read_uncached(const char *path, HANDLE fallback, u64 off, u64 len, Sink sink, void *ctx, Readback *mode);

typedef enum { DEL_DELETED, DEL_KEPT, DEL_MISSING, DEL_ERROR } DelResult;
/* Deletes `path` only if it is still the file described by `expected` and,
 * if `sha` is given, still has that SHA-256. On NT the check and the delete
 * use one handle that keeps other programs from writing to it. A read-only
 * file is deleted only with allow_readonly (its attribute is cleared first,
 * and put back if the delete fails). `why`
 * receives a heap string for DEL_KEPT and DEL_ERROR. */
DelResult plat_delete_verified(const char *path, const FStat *expected, const u8 *sha, int allow_readonly, char **why);

int plat_remove_dir(const char *path); /* 1 removed, 0 not (not empty or not a plain dir) */
int plat_delete_file(const char *path);
int plat_rename_noreplace(const char *from, const char *to);
int plat_rename_replace(const char *from, const char *to);
int plat_free_space(const char *dir, u64 *out);
u32 plat_volume_serial(const char *path);
char *plat_fs_name(const char *path); /* heap or NULL */

/* Conversions of FILETIME values. */
i64 ft_to_unix(u64 ft);
void ft_to_dos(u64 ft, u16 *time, u16 *date);
#endif
