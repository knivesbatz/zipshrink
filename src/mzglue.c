/* The two checksum functions miniz's deflate refers to; they live in miniz.c,
 * which is not vendored. Raw deflate (all ZipShrink writes) never calls them. */
#include "hash.h"
#include "miniz.h"

mz_ulong mz_crc32(mz_ulong crc, const unsigned char *ptr, size_t len) { return ptr ? crc32_update((u32)crc, ptr, len) : 0; }

mz_ulong mz_adler32(mz_ulong adler, const unsigned char *ptr, size_t len) {
    u32 a = (u32)(adler & 0xffff), b = (u32)(adler >> 16);
    if (!ptr) return 1;
    while (len) {
        size_t n = len < 5552 ? len : 5552;
        len -= n;
        while (n--) a += *ptr++, b += a;
        a %= 65521, b %= 65521;
    }
    return (mz_ulong)(b << 16 | a);
}
