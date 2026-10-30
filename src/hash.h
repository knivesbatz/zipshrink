#ifndef ZS_HASH_H
#define ZS_HASH_H
#include "base.h"

u32 crc32_update(u32 crc, const void *data, size_t n); /* start with 0 */

typedef struct {
    u32 h[8];
    u64 len;
    u8 buf[64];
    size_t fill;
} Sha256;
void sha256_init(Sha256 *s);
void sha256_update(Sha256 *s, const void *data, size_t n);
void sha256_final(Sha256 *s, u8 out[32]);

void hex_encode(const u8 *d, size_t n, char *out); /* out: 2n+1 */
int hex_decode(const char *s, u8 *out, size_t n);
#endif
