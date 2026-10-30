/* ZIP / ZIP64 records (PKWARE APPNOTE 6.3), written and parsed by hand.
 * Entries are a local header and data, no data descriptors; sizes and CRC
 * are patched into the header after the data is written, so the partial
 * archive can always be walked without a central directory. */
#ifndef ZS_ZIPFMT_H
#define ZS_ZIPFMT_H
#include "base.h"

#define ZIP_LOCAL_SIG 0x04034b50u
#define ZIP_CENTRAL_SIG 0x02014b50u
#define ZIP_EOCD_SIG 0x06054b50u
#define ZIP64_EOCD_SIG 0x06064b50u
#define ZIP64_LOCATOR_SIG 0x07064b50u
#define ZIP_FLAG_UTF8 0x0800
#define ZIP_STORE 0
#define ZIP_DEFLATE 8
#define ZIP_LOCAL_FIXED 30
#define ZIP_CENTRAL_FIXED 46
#define EXTRA_ZIP64 0x0001
#define EXTRA_NTFS 0x000a
#define EXTRA_UT 0x5455
#define EXTRA_ZSHASH 0x5a53 /* "ZSH1" + SHA-256 of the data */

typedef struct {
    const char *name; /* UTF-8, '/'-separated; folders end with '/' */
    int is_dir;
    u64 mtime, atime, btime; /* FILETIME; 0 = unknown */
    u32 attrs;               /* DOS attributes */
} EntryMeta;

typedef struct {
    u16 method;
    u32 crc;
    u64 csize, usize;
    u8 sha[32];
    int has_sha;
    int zip64_local; /* local header carries ZIP64 sizes */
} EntrySizes;

int zip_needs_zip64(u64 usize);
u64 zip_worst(u64 n); /* largest deflate output for n input bytes */

/* The local header's length depends only on meta and zip64_local, so one
 * written with zero sizes can be overwritten in place later. */
void zip_local_header(Buf *out, const EntryMeta *m, const EntrySizes *s);
void zip_central_header(Buf *out, const EntryMeta *m, const EntrySizes *s, u64 local_offset);
void zip_end_records(Buf *out, u64 entries, u64 cd_offset, u64 cd_size);

typedef struct {
    char *name; /* heap */
    u16 flags, method;
    u32 crc;
    u64 csize, usize;
    int zip64, has_sha;
    u8 sha[32];
    u64 mtime; /* FILETIME from the NTFS/UT field, 0 if none */
    u32 header_len;
} LocalInfo;
/* Parses a whole local header in p[0..n). 0 if malformed or incomplete. */
int zip_parse_local(const u8 *p, size_t n, LocalInfo *out);
/* Length of the local header starting at p, if at least 30 bytes are given. */
u32 zip_local_len(const u8 *p);

typedef struct {
    char *name;
    u16 method;
    u32 crc, ext_attrs;
    u64 csize, usize, offset;
    int has_sha;
    u8 sha[32];
} CdEntry;
/* Reads the central directory of a finished archive. */
int zip_read_cd(HANDLE h, CdEntry **out, size_t *count, char **err);
void zip_free_cd(CdEntry *e, size_t n);
#endif
