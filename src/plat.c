#include "plat.h"
#include "hash.h"
#include "failpoint.h"

#ifndef FILE_ATTRIBUTE_REPARSE_POINT
#define FILE_ATTRIBUTE_REPARSE_POINT 0x400
#endif
#ifndef FILE_ATTRIBUTE_OFFLINE
#define FILE_ATTRIBUTE_OFFLINE 0x1000
#endif
#define ATTR_RECALL_ON_OPEN 0x40000
#define ATTR_RECALL_ON_DATA_ACCESS 0x400000
#ifndef FILE_FLAG_OPEN_REPARSE_POINT
#define FILE_FLAG_OPEN_REPARSE_POINT 0x00200000
#endif
#ifndef FILE_SHARE_DELETE
#define FILE_SHARE_DELETE 0x4
#endif
#ifndef MOVEFILE_WRITE_THROUGH
#define MOVEFILE_WRITE_THROUGH 8
#endif
#define DELETE_ACCESS 0x00010000

/* Wide-character entry points, resolved on NT only. */
typedef HANDLE(WINAPI *CreateFileW_t)(const WCHAR *, DWORD, DWORD, void *, DWORD, DWORD, HANDLE);
typedef DWORD(WINAPI *GetFileAttributesW_t)(const WCHAR *);
typedef HANDLE(WINAPI *FindFirstFileW_t)(const WCHAR *, WIN32_FIND_DATAW *);
typedef BOOL(WINAPI *FindNextFileW_t)(HANDLE, WIN32_FIND_DATAW *);
typedef BOOL(WINAPI *PathW_t)(const WCHAR *);
typedef BOOL(WINAPI *MoveFileExW_t)(const WCHAR *, const WCHAR *, DWORD);
typedef DWORD(WINAPI *GetFullPathNameW_t)(const WCHAR *, DWORD, WCHAR *, WCHAR **);
typedef BOOL(WINAPI *GetDiskFreeSpaceExW_t)(const WCHAR *, ULARGE_INTEGER *, ULARGE_INTEGER *, ULARGE_INTEGER *);
typedef BOOL(WINAPI *GetDiskFreeSpaceExA_t)(const char *, ULARGE_INTEGER *, ULARGE_INTEGER *, ULARGE_INTEGER *);
typedef BOOL(WINAPI *GetVolumeInformationW_t)(const WCHAR *, WCHAR *, DWORD, DWORD *, DWORD *, DWORD *, WCHAR *, DWORD);
typedef LONG(WINAPI *NtSetInformationFile_t)(HANDLE, void *, void *, ULONG, int);

static struct {
    int ready;
    CreateFileW_t CreateFileW;
    GetFileAttributesW_t GetFileAttributesW;
    FindFirstFileW_t FindFirstFileW;
    FindNextFileW_t FindNextFileW;
    PathW_t DeleteFileW, RemoveDirectoryW;
    MoveFileExW_t MoveFileExW;
    GetFullPathNameW_t GetFullPathNameW;
    GetDiskFreeSpaceExW_t GetDiskFreeSpaceExW;
    GetDiskFreeSpaceExA_t GetDiskFreeSpaceExA;
    GetVolumeInformationW_t GetVolumeInformationW;
    NtSetInformationFile_t NtSetInformationFile;
} W;

static int wide(void) {
    if (!W.ready) {
        W.ready = 1;
        W.GetDiskFreeSpaceExA = (GetDiskFreeSpaceExA_t)zs_proc("kernel32.dll", "GetDiskFreeSpaceExA");
        if (zs_is_nt()) {
            W.CreateFileW = (CreateFileW_t)zs_proc("kernel32.dll", "CreateFileW");
            W.GetFileAttributesW = (GetFileAttributesW_t)zs_proc("kernel32.dll", "GetFileAttributesW");
            W.FindFirstFileW = (FindFirstFileW_t)zs_proc("kernel32.dll", "FindFirstFileW");
            W.FindNextFileW = (FindNextFileW_t)zs_proc("kernel32.dll", "FindNextFileW");
            W.DeleteFileW = (PathW_t)zs_proc("kernel32.dll", "DeleteFileW");
            W.RemoveDirectoryW = (PathW_t)zs_proc("kernel32.dll", "RemoveDirectoryW");
            W.MoveFileExW = (MoveFileExW_t)zs_proc("kernel32.dll", "MoveFileExW");
            W.GetFullPathNameW = (GetFullPathNameW_t)zs_proc("kernel32.dll", "GetFullPathNameW");
            W.GetDiskFreeSpaceExW = (GetDiskFreeSpaceExW_t)zs_proc("kernel32.dll", "GetDiskFreeSpaceExW");
            W.GetVolumeInformationW = (GetVolumeInformationW_t)zs_proc("kernel32.dll", "GetVolumeInformationW");
            W.NtSetInformationFile = (NtSetInformationFile_t)zs_proc("ntdll.dll", "NtSetInformationFile");
        }
    }
    return W.CreateFileW != NULL;
}

/* Path for the wide API: \\?\ prefix lifts the 260-character limit (NT). */
static WCHAR *wpath(const char *path) {
    char *p;
    WCHAR *w;
    if (zs_starts_with(path, "\\\\?\\") || strlen(path) < 240) return zs_utf8_to_wide(path);
    p = zs_starts_with(path, "\\\\") ? zs_join("\\\\?\\UNC\\", path + 2) : zs_join("\\\\?\\", path);
    w = zs_utf8_to_wide(p);
    zs_free(p);
    return w;
}

/* Path for the ANSI API, or NULL (ERROR_INVALID_NAME) if it cannot be represented. */
static char *apath(const char *path) {
    int lossy = 0;
    char *a = zs_utf8_to_acp(path, &lossy);
    if (lossy || strlen(a) >= MAX_PATH) {
        zs_free(a);
        SetLastError(lossy ? ERROR_INVALID_NAME : ERROR_FILENAME_EXCED_RANGE);
        return NULL;
    }
    return a;
}

const char *plat_error(DWORD code) {
    static char out[512];
    char msg[400];
    DWORD n;
    if (code == ERROR_INVALID_NAME && !zs_is_nt()) return "the name cannot be represented on this version of Windows";
    n = FormatMessageA(FORMAT_MESSAGE_FROM_SYSTEM | FORMAT_MESSAGE_IGNORE_INSERTS, NULL, code, 0, msg, sizeof(msg), NULL);
    while (n > 0 && (msg[n - 1] == '\r' || msg[n - 1] == '\n' || msg[n - 1] == ' ' || msg[n - 1] == '.')) n--;
    msg[n] = 0;
    {
        char *u = zs_acp_to_utf8(n ? msg : "unknown error");
        char *f = zs_fmt("%s (error %u)", u, (unsigned)code);
        size_t k = strlen(f);
        if (k >= sizeof(out)) k = sizeof(out) - 1;
        memcpy(out, f, k);
        out[k] = 0;
        zs_free(u);
        zs_free(f);
    }
    return out;
}

int plat_is_disk_full(DWORD c) { return c == ERROR_DISK_FULL || c == ERROR_HANDLE_DISK_FULL; }
int plat_is_sharing(DWORD c) { return c == ERROR_SHARING_VIOLATION || c == ERROR_LOCK_VIOLATION; }

static u64 ft64(FILETIME f) { return ((u64)f.dwHighDateTime << 32) | f.dwLowDateTime; }

int fstat_same(const FStat *a, const FStat *b) {
    if (a->size != b->size || a->mtime != b->mtime) return 0;
    /* File ids and volume serials are reliable on NT (NTFS and FAT). */
    if (zs_is_nt() && (a->volume != b->volume || a->index != b->index)) return 0;
    return 1;
}

const char *fstat_difference(const FStat *was, const FStat *now) {
    if (zs_is_nt() && (was->volume != now->volume || was->index != now->index)) return "it was replaced by a different file";
    if (was->size != now->size) return "its size changed";
    if (was->mtime != now->mtime) return "its modification time changed";
    return "it changed";
}

static Kind kind_of_attrs(u32 a) {
    if (a & FILE_ATTRIBUTE_REPARSE_POINT) return K_LINK;
    if (a & (FILE_ATTRIBUTE_OFFLINE | ATTR_RECALL_ON_OPEN | ATTR_RECALL_ON_DATA_ACCESS)) return K_OTHER;
    if (a & FILE_ATTRIBUTE_DIRECTORY) return K_DIR;
    return K_FILE;
}

int plat_kind(const char *path, Kind *kind, FStat *st) {
    /* FindFirstFile on the path itself returns its attributes, size and times
     * without opening it (works for files and folders, on every version). */
    HANDLE f;
    char *trimmed = zs_strdup(path);
    size_t n = strlen(trimmed);
    while (n > 3 && (trimmed[n - 1] == '\\' || trimmed[n - 1] == '/')) trimmed[--n] = 0;
    memset(st, 0, sizeof(*st));
    if (wide()) {
        WIN32_FIND_DATAW d;
        WCHAR *w = wpath(trimmed);
        f = W.FindFirstFileW(w, &d);
        zs_free(w);
        if (f == INVALID_HANDLE_VALUE) goto missing;
        FindClose(f);
        st->attrs = d.dwFileAttributes;
        st->size = ((u64)d.nFileSizeHigh << 32) | d.nFileSizeLow;
        st->mtime = ft64(d.ftLastWriteTime), st->atime = ft64(d.ftLastAccessTime), st->btime = ft64(d.ftCreationTime);
    } else {
        WIN32_FIND_DATAA d;
        char *a = apath(trimmed);
        if (!a) goto missing;
        f = FindFirstFileA(a, &d);
        zs_free(a);
        if (f == INVALID_HANDLE_VALUE) goto missing;
        FindClose(f);
        st->attrs = d.dwFileAttributes;
        st->size = ((u64)d.nFileSizeHigh << 32) | d.nFileSizeLow;
        st->mtime = ft64(d.ftLastWriteTime), st->atime = ft64(d.ftLastAccessTime), st->btime = ft64(d.ftCreationTime);
    }
    zs_free(trimmed);
    *kind = kind_of_attrs(st->attrs);
    return 1;
missing: {
    DWORD e = GetLastError();
    zs_free(trimmed);
    /* FindFirstFile fails on drive roots; GetFileAttributes does not. */
    if (wide()) {
        WCHAR *w = wpath(path);
        DWORD a = W.GetFileAttributesW(w);
        zs_free(w);
        if (a != INVALID_FILE_ATTRIBUTES) return st->attrs = a, *kind = kind_of_attrs(a), 1;
    } else {
        char *ap = apath(path);
        DWORD a = ap ? GetFileAttributesA(ap) : INVALID_FILE_ATTRIBUTES;
        zs_free(ap);
        if (a != INVALID_FILE_ATTRIBUTES) return st->attrs = a, *kind = kind_of_attrs(a), 1;
    }
    *kind = K_MISSING;
    SetLastError(e);
    return e == ERROR_FILE_NOT_FOUND || e == ERROR_PATH_NOT_FOUND || e == ERROR_NO_MORE_FILES ? 1 : 0;
}
}

int plat_exists(const char *path) {
    Kind k;
    FStat st;
    return plat_kind(path, &k, &st) && k != K_MISSING;
}

char *plat_full_path(const char *path) {
    if (wide()) {
        WCHAR *w = zs_utf8_to_wide(path), buf[1024], *fp;
        DWORD n = W.GetFullPathNameW(w, 1024, buf, &fp);
        zs_free(w);
        if (n == 0 || n >= 1024) return NULL;
        return zs_wide_to_utf8(buf);
    } else {
        char *a = apath(path), buf[MAX_PATH], *fp;
        DWORD n;
        if (!a) return NULL;
        n = GetFullPathNameA(a, MAX_PATH, buf, &fp);
        zs_free(a);
        if (n == 0 || n >= MAX_PATH) return NULL;
        return zs_acp_to_utf8(buf);
    }
}

static int name_order(const void *a, const void *b) {
    const DirEntry *x = a, *y = b;
    if (!x->name || !y->name) return x->name ? -1 : y->name ? 1 : 0;
    return zs_strcmp(x->name, y->name);
}

static void sort_entries(DirEntry *e, size_t n) {
    size_t i, j;
    /* Insertion sort: folders hold modest numbers of entries per level. */
    for (i = 1; i < n; i++) {
        DirEntry t = e[i];
        for (j = i; j > 0 && name_order(&e[j - 1], &t) > 0; j--) e[j] = e[j - 1];
        e[j] = t;
    }
}

static int has_question_mark(const char *s) {
    for (; *s; s++)
        if (*s == '?') return 1;
    return 0;
}

int plat_list_dir(const char *dir, DirEntry **out, size_t *count) {
    DirEntry *v = NULL;
    size_t n = 0, cap = 0;
    char *pattern = zs_path_join(dir, "*");
    HANDLE f;
    int ok = 1;
    if (wide()) {
        WIN32_FIND_DATAW d;
        WCHAR *w = wpath(pattern);
        f = W.FindFirstFileW(w, &d);
        zs_free(w);
        if (f == INVALID_HANDLE_VALUE) ok = GetLastError() == ERROR_FILE_NOT_FOUND;
        else {
            do {
                if ((d.cFileName[0] == '.' && d.cFileName[1] == 0) || (d.cFileName[0] == '.' && d.cFileName[1] == '.' && d.cFileName[2] == 0)) continue;
                if (n == cap) cap = cap ? cap * 2 : 32, v = zs_realloc(v, cap * sizeof(*v));
                v[n].name = zs_wide_to_utf8(d.cFileName);
                {
                    /* A name with an unpaired surrogate has no UTF-8 form. */
                    WCHAR *back = zs_utf8_to_wide(v[n].name);
                    size_t k = 0;
                    while (back[k] && back[k] == d.cFileName[k]) k++;
                    if (back[k] != d.cFileName[k]) zs_free(v[n].name), v[n].name = NULL;
                    zs_free(back);
                }
                v[n].attrs = d.dwFileAttributes;
                v[n].size = ((u64)d.nFileSizeHigh << 32) | d.nFileSizeLow;
                v[n].mtime = ft64(d.ftLastWriteTime), v[n].atime = ft64(d.ftLastAccessTime), v[n].btime = ft64(d.ftCreationTime);
                n++;
            } while (W.FindNextFileW(f, &d));
            ok = GetLastError() == ERROR_NO_MORE_FILES;
            FindClose(f);
        }
    } else {
        WIN32_FIND_DATAA d;
        char *a = apath(pattern);
        f = a ? FindFirstFileA(a, &d) : INVALID_HANDLE_VALUE;
        zs_free(a);
        if (f == INVALID_HANDLE_VALUE) ok = GetLastError() == ERROR_FILE_NOT_FOUND;
        else {
            do {
                if (!lstrcmpA(d.cFileName, ".") || !lstrcmpA(d.cFileName, "..")) continue;
                if (n == cap) cap = cap ? cap * 2 : 32, v = zs_realloc(v, cap * sizeof(*v));
                /* A '?' in an ANSI name means a character the code page lacks. */
                v[n].name = has_question_mark(d.cFileName) ? NULL : zs_acp_to_utf8(d.cFileName);
                v[n].attrs = d.dwFileAttributes;
                v[n].size = ((u64)d.nFileSizeHigh << 32) | d.nFileSizeLow;
                v[n].mtime = ft64(d.ftLastWriteTime), v[n].atime = ft64(d.ftLastAccessTime), v[n].btime = ft64(d.ftCreationTime);
                n++;
            } while (FindNextFileA(f, &d));
            ok = GetLastError() == ERROR_NO_MORE_FILES;
            FindClose(f);
        }
    }
    zs_free(pattern);
    sort_entries(v, n);
    *out = v;
    *count = n;
    return ok;
}

void plat_free_entries(DirEntry *e, size_t n) {
    size_t i;
    for (i = 0; i < n; i++) zs_free(e[i].name);
    zs_free(e);
}

static HANDLE open_any(const char *path, DWORD access, DWORD share, DWORD disp, DWORD flags) {
    HANDLE h;
    if (wide()) {
        WCHAR *w = wpath(path);
        h = W.CreateFileW(w, access, share, NULL, disp, flags, NULL);
        zs_free(w);
    } else {
        char *a = apath(path);
        if (!a) return INVALID_HANDLE_VALUE;
        /* Windows 9x rejects the NT-only flags. */
        h = CreateFileA(a, access & ~DELETE_ACCESS, share & ~FILE_SHARE_DELETE, NULL, disp, flags & ~(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS), NULL);
        zs_free(a);
    }
    return h;
}

HANDLE plat_open_read(const char *path, int deny_writers) {
    DWORD share = deny_writers ? FILE_SHARE_READ : FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE;
    HANDLE h = open_any(path, GENERIC_READ, share, OPEN_EXISTING, FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_SEQUENTIAL_SCAN);
    if (h != INVALID_HANDLE_VALUE) {
        BY_HANDLE_FILE_INFORMATION bi;
        if (GetFileInformationByHandle(h, &bi) && (bi.dwFileAttributes & (FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT))) {
            CloseHandle(h);
            SetLastError(ERROR_ACCESS_DENIED);
            return INVALID_HANDLE_VALUE;
        }
    }
    return h;
}

HANDLE plat_create_new(const char *path) { return open_any(path, GENERIC_READ | GENERIC_WRITE, FILE_SHARE_READ, CREATE_NEW, FILE_ATTRIBUTE_NORMAL); }
HANDLE plat_open_rw(const char *path) { return open_any(path, GENERIC_READ | GENERIC_WRITE, FILE_SHARE_READ, OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL); }

int plat_stat_handle(HANDLE h, FStat *st) {
    BY_HANDLE_FILE_INFORMATION bi;
    if (!GetFileInformationByHandle(h, &bi)) return 0;
    st->size = ((u64)bi.nFileSizeHigh << 32) | bi.nFileSizeLow;
    st->mtime = ft64(bi.ftLastWriteTime), st->atime = ft64(bi.ftLastAccessTime), st->btime = ft64(bi.ftCreationTime);
    st->volume = bi.dwVolumeSerialNumber;
    st->index = ((u64)bi.nFileIndexHigh << 32) | bi.nFileIndexLow;
    st->attrs = bi.dwFileAttributes;
    return 1;
}

static int seek(HANDLE h, u64 off) {
    LONG hi = (LONG)(off >> 32);
    DWORD lo = SetFilePointer(h, (LONG)(u32)off, &hi, FILE_BEGIN);
    return !(lo == 0xFFFFFFFF && GetLastError() != NO_ERROR);
}

int plat_pwrite(HANDLE h, const void *buf, u32 n, u64 off) {
    DWORD done;
    const u8 *p = buf;
    if (!seek(h, off)) return 0;
    while (n) {
        if (!WriteFile(h, p, n, &done, NULL)) return 0;
        if (done == 0) return SetLastError(ERROR_HANDLE_DISK_FULL), 0;
        p += done, n -= done;
    }
    return 1;
}

int plat_pread(HANDLE h, void *buf, u32 n, u64 off, u32 *got) {
    DWORD done;
    if (!seek(h, off)) return 0;
    if (!ReadFile(h, buf, n, &done, NULL)) return 0;
    *got = done;
    return 1;
}

int plat_set_len(HANDLE h, u64 len) { return seek(h, len) && SetEndOfFile(h); }

u64 plat_file_len(HANDLE h) {
    DWORD hi = 0, lo = GetFileSize(h, &hi);
    if (lo == 0xFFFFFFFF && GetLastError() != NO_ERROR) return 0;
    return ((u64)hi << 32) | lo;
}

int plat_flush(HANDLE h) { return FlushFileBuffers(h) != 0; }

int plat_lock(HANDLE h) {
    if (LockFile(h, 0, 0, 1, 0)) return 1;
    {
        DWORD e = GetLastError();
        /* Network or FAT drives without locking support: proceed unlocked. */
        if (e == ERROR_NOT_SUPPORTED || e == ERROR_INVALID_FUNCTION) return 1;
        SetLastError(e);
    }
    return 0;
}

int plat_read_uncached(const char *path, HANDLE fallback, u64 off, u64 len, Sink sink, void *ctx, Readback *mode) {
    enum { ALIGN = 4096, CHUNK = 1 << 20 };
    u8 *mem;
    u64 end = off + len, pos;
    HANDLE d = INVALID_HANDLE_VALUE;
    int ok = 1;
    if (zs_is_nt())
        d = open_any(path, GENERIC_READ, FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, OPEN_EXISTING, FILE_FLAG_NO_BUFFERING);
    mem = VirtualAlloc(NULL, CHUNK, MEM_COMMIT, PAGE_READWRITE); /* page aligned */
    if (!mem) return 0;
    if (d != INVALID_HANDLE_VALUE) {
        *mode = RB_UNCACHED;
        pos = off - off % ALIGN;
        while (pos < end) {
            u32 got = 0;
            if (!plat_pread(d, mem, CHUNK, pos, &got)) {
                /* ERROR_INVALID_PARAMETER: the volume needs another alignment. */
                if (GetLastError() == ERROR_INVALID_PARAMETER && pos == off - off % ALIGN) {
                    CloseHandle(d);
                    d = INVALID_HANDLE_VALUE;
                    goto cached;
                }
                ok = 0;
                break;
            }
            if (got == 0) {
                SetLastError(ERROR_HANDLE_EOF);
                ok = 0;
                break;
            }
            {
                u64 from = off > pos ? off : pos, to = end < pos + got ? end : pos + got;
                if (to > from && !sink(ctx, mem + (from - pos), (u32)(to - from))) {
                    ok = 0;
                    break;
                }
            }
            pos += got;
        }
        CloseHandle(d);
        VirtualFree(mem, 0, MEM_RELEASE);
        return ok;
    }
cached:
    /* Windows 9x: no cache bypass is available to programs. */
    *mode = RB_CACHED;
    pos = off;
    while (pos < end) {
        u32 want = end - pos < CHUNK ? (u32)(end - pos) : CHUNK, got = 0;
        if (!plat_pread(fallback, mem, want, pos, &got) || got != want) {
            if (got != want) SetLastError(ERROR_HANDLE_EOF);
            ok = 0;
            break;
        }
        if (!sink(ctx, mem, got)) {
            ok = 0;
            break;
        }
        pos += got;
    }
    VirtualFree(mem, 0, MEM_RELEASE);
    return ok;
}

static int hash_handle(HANDLE h, u8 out[32], u64 *n) {
    Sha256 s;
    u8 *buf = zs_alloc(1 << 16);
    u64 pos = 0;
    sha256_init(&s);
    for (;;) {
        u32 got = 0;
        if (!plat_pread(h, buf, 1 << 16, pos, &got)) {
            zs_free(buf);
            return 0;
        }
        if (!got) break;
        sha256_update(&s, buf, got);
        pos += got;
    }
    sha256_final(&s, out);
    *n = pos;
    zs_free(buf);
    return 1;
}

typedef struct {
    LONG status;
    ULONG_PTR info;
} IoStatus;

static void set_attrs(const char *path, u32 attrs) {
    char *a = apath(path); /* Windows 9x only */
    if (a) SetFileAttributesA(a, attrs ? attrs : FILE_ATTRIBUTE_NORMAL);
    zs_free(a);
}

DelResult plat_delete_verified(const char *path, const FStat *expected, const u8 *sha, int allow_readonly, char **why) {
    HANDLE h;
    FStat now;
    *why = NULL;
    if (zs_fail("delete_source")) {
        *why = zs_strdup("kept because it could not be deleted: injected error");
        return DEL_KEPT;
    }
    if (wide() && W.NtSetInformationFile) {
        /* One handle: denies writers, verifies, then marks for deletion. */
        WCHAR *w = wpath(path);
        h = W.CreateFileW(w, GENERIC_READ | DELETE_ACCESS | FILE_WRITE_ATTRIBUTES, FILE_SHARE_READ, NULL, OPEN_EXISTING, FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_SEQUENTIAL_SCAN, NULL);
        zs_free(w);
    } else {
        h = open_any(path, GENERIC_READ, FILE_SHARE_READ, OPEN_EXISTING, FILE_FLAG_SEQUENTIAL_SCAN);
    }
    if (h == INVALID_HANDLE_VALUE) {
        DWORD e = GetLastError();
        if (e == ERROR_FILE_NOT_FOUND || e == ERROR_PATH_NOT_FOUND) return DEL_MISSING;
        if (plat_is_sharing(e)) {
            *why = zs_strdup("kept because another program has it open");
            return DEL_KEPT;
        }
        *why = zs_fmt("kept because it could not be opened: %s", plat_error(e));
        return DEL_KEPT;
    }
    if (!plat_stat_handle(h, &now)) {
        *why = zs_fmt("kept because it could not be examined: %s", plat_error(GetLastError()));
        CloseHandle(h);
        return DEL_KEPT;
    }
    if (now.attrs & (FILE_ATTRIBUTE_REPARSE_POINT | FILE_ATTRIBUTE_DIRECTORY)) {
        *why = zs_strdup("kept because it was replaced by a link or folder");
        CloseHandle(h);
        return DEL_KEPT;
    }
    if (!fstat_same(expected, &now)) {
        *why = zs_fmt("kept because %s after it was archived", fstat_difference(expected, &now));
        CloseHandle(h);
        return DEL_KEPT;
    }
    if (sha) {
        u8 got[32];
        u64 n;
        if (!hash_handle(h, got, &n)) {
            *why = zs_fmt("kept because it could not be read again: %s", plat_error(GetLastError()));
            CloseHandle(h);
            return DEL_KEPT;
        }
        if (n != expected->size || memcmp(got, sha, 32) != 0) {
            *why = zs_strdup("kept because its contents no longer match the archived copy");
            CloseHandle(h);
            return DEL_KEPT;
        }
    }
    if (wide() && W.NtSetInformationFile) {
        IoStatus io;
        BOOLEAN del = TRUE;
        LONG st;
        int cleared = 0;
        if ((now.attrs & FILE_ATTRIBUTE_READONLY) && allow_readonly) {
            /* FileBasicInformation: times 0 = unchanged; attributes without
             * read-only (FILE_ATTRIBUTE_NORMAL when nothing else is left). */
            struct {
                LARGE_INTEGER c, a, w, ch;
                ULONG attrs;
            } bi;
            memset(&bi, 0, sizeof(bi));
            bi.attrs = (now.attrs & ~(u32)FILE_ATTRIBUTE_READONLY) ? (now.attrs & ~(u32)FILE_ATTRIBUTE_READONLY) : FILE_ATTRIBUTE_NORMAL;
            cleared = W.NtSetInformationFile(h, &io, &bi, sizeof(bi), 4 /* FileBasicInformation */) == 0;
        }
        st = W.NtSetInformationFile(h, &io, &del, sizeof(del), 13 /* FileDispositionInformation */);
        if (st != 0 && cleared) {
            struct {
                LARGE_INTEGER c, a, w, ch;
                ULONG attrs;
            } bi;
            memset(&bi, 0, sizeof(bi));
            bi.attrs = now.attrs;
            W.NtSetInformationFile(h, &io, &bi, sizeof(bi), 4);
        }
        CloseHandle(h); /* the file goes when the last handle closes */
        if (st == 0) return DEL_DELETED;
        if ((u32)st == 0xC0000121u /* STATUS_CANNOT_DELETE */) {
            *why = zs_strdup("kept because it is read-only");
            return DEL_KEPT;
        }
        *why = zs_fmt("kept because it could not be deleted (status 0x%08x)", (unsigned)st);
        return DEL_KEPT;
    }
    /* Windows 9x: no handle-based delete. The handle kept writers out while
     * the file was checked; check its identity once more, then delete. */
    CloseHandle(h);
    {
        Kind k;
        FStat again;
        if (!plat_kind(path, &k, &again) || k != K_FILE || again.size != expected->size || again.mtime != expected->mtime) {
            *why = zs_strdup("kept because it changed while it was being checked");
            return DEL_KEPT;
        }
    }
    if ((now.attrs & FILE_ATTRIBUTE_READONLY) && allow_readonly) set_attrs(path, now.attrs & ~(u32)FILE_ATTRIBUTE_READONLY);
    if (!plat_delete_file(path)) {
        DWORD e = GetLastError();
        if ((now.attrs & FILE_ATTRIBUTE_READONLY) && allow_readonly) set_attrs(path, now.attrs);
        *why = e == ERROR_ACCESS_DENIED && (now.attrs & FILE_ATTRIBUTE_READONLY) ? zs_strdup("kept because it is read-only") : zs_fmt("kept because it could not be deleted: %s", plat_error(e));
        return DEL_KEPT;
    }
    return DEL_DELETED;
}

int plat_delete_file(const char *path) {
    if (wide()) {
        WCHAR *w = wpath(path);
        BOOL ok = W.DeleteFileW(w);
        zs_free(w);
        return ok;
    } else {
        char *a = apath(path);
        BOOL ok = a ? DeleteFileA(a) : FALSE;
        zs_free(a);
        return ok;
    }
}

int plat_remove_dir(const char *path) {
    Kind k;
    FStat st;
    if (!plat_kind(path, &k, &st) || k != K_DIR) return 0;
    if (wide()) {
        WCHAR *w = wpath(path);
        BOOL ok = W.RemoveDirectoryW(w);
        zs_free(w);
        return ok;
    } else {
        char *a = apath(path);
        BOOL ok = a ? RemoveDirectoryA(a) : FALSE;
        zs_free(a);
        return ok;
    }
}

static int rename_impl(const char *from, const char *to, int replace) {
    if (wide() && W.MoveFileExW) {
        WCHAR *a = wpath(from), *b = wpath(to);
        BOOL ok = W.MoveFileExW(a, b, MOVEFILE_WRITE_THROUGH | (replace ? MOVEFILE_REPLACE_EXISTING : 0));
        zs_free(a);
        zs_free(b);
        return ok;
    } else {
        char *a = apath(from), *b = apath(to);
        BOOL ok = FALSE;
        if (a && b) {
            /* MoveFile never replaces; Windows 9x has no atomic replace, so
             * the old file goes first (only after the user confirmed it). */
            if (replace && plat_exists(to)) DeleteFileA(b);
            ok = MoveFileA(a, b);
        }
        zs_free(a);
        zs_free(b);
        return ok;
    }
}

int plat_rename_noreplace(const char *from, const char *to) {
    if (plat_exists(to)) return SetLastError(ERROR_ALREADY_EXISTS), 0;
    return rename_impl(from, to, 0);
}

int plat_rename_replace(const char *from, const char *to) { return rename_impl(from, to, 1); }

/* "C:\", or "\\server\share\" for network paths. */
static char *root_of(const char *path) {
    char *full = plat_full_path(path);
    char *r;
    if (!full) full = zs_strdup(path);
    if (full[0] && full[1] == ':') {
        char drive[4] = {full[0], ':', '\\', 0};
        r = zs_strdup(drive);
    }
    else if (zs_starts_with(full, "\\\\")) {
        const char *p = full + 2;
        int slashes = 0;
        while (*p && slashes < 2) {
            if (*p == '\\') slashes++;
            if (slashes < 2) p++;
        }
        {
            char *t = zs_strndup(full, (size_t)(p - full));
            r = zs_join(t, "\\");
            zs_free(t);
        }
    } else r = zs_strdup(full);
    zs_free(full);
    return r;
}

int plat_free_space(const char *dir, u64 *out) {
    ULARGE_INTEGER avail, total, free_;
    wide();
    if (W.GetDiskFreeSpaceExW) {
        WCHAR *w = zs_utf8_to_wide(dir);
        BOOL ok = W.GetDiskFreeSpaceExW(w, &avail, &total, &free_);
        zs_free(w);
        if (ok) return *out = avail.QuadPart, 1;
    }
    if (W.GetDiskFreeSpaceExA) {
        char *a = apath(dir);
        BOOL ok = a && W.GetDiskFreeSpaceExA(a, &avail, &total, &free_);
        zs_free(a);
        if (ok) return *out = avail.QuadPart, 1;
    }
    {
        /* Windows 95 before OSR2: cluster arithmetic, capped near 2 GB. */
        char *r = root_of(dir), *a = apath(r);
        DWORD spc, bps, fc, tc;
        BOOL ok = a && GetDiskFreeSpaceA(a, &spc, &bps, &fc, &tc);
        zs_free(r);
        zs_free(a);
        if (ok) return *out = (u64)spc * bps * fc, 1;
    }
    return 0;
}

u32 plat_volume_serial(const char *path) {
    char *r = root_of(path);
    DWORD serial = 0;
    if (wide() && W.GetVolumeInformationW) {
        WCHAR *w = zs_utf8_to_wide(r);
        W.GetVolumeInformationW(w, NULL, 0, &serial, NULL, NULL, NULL, 0);
        zs_free(w);
    } else {
        char *a = apath(r);
        if (a) GetVolumeInformationA(a, NULL, 0, &serial, NULL, NULL, NULL, 0);
        zs_free(a);
    }
    zs_free(r);
    return serial;
}

char *plat_fs_name(const char *path) {
    char *r = root_of(path), *a = apath(r), fs[64];
    char *res = NULL;
    if (a && GetVolumeInformationA(a, NULL, 0, NULL, NULL, NULL, fs, sizeof(fs))) res = zs_strdup(fs);
    zs_free(r);
    zs_free(a);
    return res;
}

i64 ft_to_unix(u64 ft) { return (i64)(ft / 10000000ULL) - 11644473600LL; }

void ft_to_dos(u64 ft, u16 *time, u16 *date) {
    FILETIME f, local;
    WORD d = 0x21, t = 0; /* 1980-01-01 */
    f.dwLowDateTime = (DWORD)ft;
    f.dwHighDateTime = (DWORD)(ft >> 32);
    if (ft && FileTimeToLocalFileTime(&f, &local) && FileTimeToDosDateTime(&local, &d, &t)) {
        *time = t;
        *date = d;
        return;
    }
    *time = 0;
    *date = 0x21;
}
