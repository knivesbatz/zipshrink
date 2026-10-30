/* The recovery journal: an append-only, checksummed, flushed text file next
 * to the partial archive. Each line is
 *     <crc32 of the rest, 8 hex digits> <type>[\t<key>=<value>]...\n
 * with \, tab, CR and LF escaped in values. A crash can only tear the last
 * line, which is detected and ignored; damage anywhere else stops the job. */
#ifndef ZS_JOURNAL_H
#define ZS_JOURNAL_H
#include "base.h"

typedef struct {
    char type;
    int n;
    char **key, **val;
} Rec;

const char *rec_get(const Rec *r, const char *key);
u64 rec_u64(const Rec *r, const char *key);
int rec_has(const Rec *r, const char *key);

/* Building a record. */
void rec_begin(Buf *b, char type);
void rec_str(Buf *b, const char *key, const char *val);
void rec_num(Buf *b, const char *key, u64 val);

typedef struct {
    HANDLE h;
    char *path;
    Buf pending;
    u64 len;
} Journal;

int journal_create(Journal *j, const char *path);
int journal_reopen(Journal *j, const char *path, u64 valid_len, int *busy);
void journal_add(Journal *j, Buf *rec); /* takes the record (rec is reset) */
int journal_sync(Journal *j);           /* writes pending records and flushes */
void journal_close(Journal *j);

typedef struct {
    Rec *recs;
    size_t count;
    u64 valid_len;
    u64 first_len; /* bytes of the header line */
    int torn_tail;
} Loaded;
/* 0 on error; *err is set ("damaged at byte N", "not a journal", I/O). */
int journal_load(const char *path, Loaded *out, char **err, int *busy);
void journal_free_loaded(Loaded *l);
void rec_free(Rec *r);
#endif
