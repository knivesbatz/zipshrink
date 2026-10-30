#ifndef ZS_VERIFY_H
#define ZS_VERIFY_H
#include "engine.h"
#include "hash.h"
#include "miniz.h"

/* Streaming inflate (or store) that hashes the output. */
typedef struct {
    u16 method;
    tinfl_decompressor *z;
    u8 *dict;
    size_t dict_ofs;
    u32 crc;
    Sha256 sha;
    u64 out, in;
    int ended, trailing, corrupt;
} Inflater;
void inflater_init(Inflater *f, u16 method);
int inflater_feed(Inflater *f, const u8 *data, size_t n); /* 0 if corrupt */
/* Completes; ok = stream ended exactly at the end of the input. */
void inflater_finish(Inflater *f, u32 *crc, u8 sha[32], u64 *out, int *ok);
void inflater_free(Inflater *f);

typedef struct {
    const char *name;
    u64 offset, end;
    u16 method;
    u32 crc;
    u64 csize, usize;
    const u8 *sha; /* NULL for folders */
    int zip64_local;
    u32 header_crc;
} Expected;

typedef enum { CHK_OK, CHK_IO, CHK_MISMATCH } CheckResult;
/* Reads one entry back from the device and compares everything with what
 * was written. *why receives a heap message on failure. */
CheckResult check_entry(HANDLE h, const char *path, const Expected *x, Readback *mode, char **why);
#endif
