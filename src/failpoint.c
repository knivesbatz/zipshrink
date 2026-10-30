#include "failpoint.h"
#ifdef ZS_FAILPOINTS

typedef struct {
    char name[48];
    u32 nth, hits;
    int repeat, action, armed;
    ZsHook hook;
    void *ctx;
} Point;

static Point points[64];
static int env_loaded;

static Point *find(const char *name, int create) {
    int i;
    for (i = 0; i < (int)ARRAY_LEN(points); i++)
        if (points[i].name[0] && !zs_strcmp(points[i].name, name)) return &points[i];
    if (!create) return NULL;
    for (i = 0; i < (int)ARRAY_LEN(points); i++)
        if (!points[i].name[0]) {
            size_t n = strlen(name);
            if (n >= sizeof(points[i].name)) n = sizeof(points[i].name) - 1;
            memcpy(points[i].name, name, n);
            points[i].name[n] = 0;
            return &points[i];
        }
    return NULL;
}

void zs_fp_arm(const char *name, u32 nth, int repeat, int action) {
    Point *p = find(name, 1);
    if (!p) return;
    p->nth = nth ? nth : 1, p->repeat = repeat, p->action = action, p->armed = 1, p->hits = 0;
}

void zs_fp_hook(const char *name, ZsHook fn, void *ctx) {
    Point *p = find(name, 1);
    if (p) p->hook = fn, p->ctx = ctx;
}

void zs_fp_reset(void) {
    memset(points, 0, sizeof(points));
    env_loaded = 1;
}

static void load_env(void) {
    char *spec, *p;
    env_loaded = 1;
    spec = zs_getenv("ZIPSHRINK_FAILPOINT");
    if (!spec) return;
    for (p = spec; *p;) {
        char *item = p, *end, *colon, *eq;
        u64 nth = 1;
        int repeat = 0, action = FP_ABORT;
        while (*p && *p != ',') p++;
        end = p;
        if (*p) *p++ = 0;
        eq = NULL, colon = NULL;
        {
            char *q;
            for (q = item; q < end; q++) {
                if (*q == '=' && !eq) eq = q;
                if (*q == ':' && !colon) colon = q;
            }
        }
        if (eq) {
            *eq = 0;
            if (!zs_strcmp(eq + 1, "enospc")) action = FP_ENOSPC;
            else if (!zs_strcmp(eq + 1, "eio")) action = FP_EIO;
        }
        if (colon) {
            size_t n;
            *colon = 0;
            n = strlen(colon + 1);
            if (n && colon[n] == '+') repeat = 1, colon[n] = 0;
            zs_parse_u64(colon + 1, &nth);
        }
        zs_fp_arm(item, (u32)nth, repeat, action);
    }
    zs_free(spec);
}

static void die(const char *name) {
    zs_errf("failpoint %s: aborting\n", name);
    TerminateProcess(GetCurrentProcess(), ZS_FP_EXIT);
}

int zs_fail(const char *name) {
    Point *p;
    if (!env_loaded) load_env();
    p = find(name, 0);
    if (!p) return 0;
    if (p->hook) p->hook(p->ctx);
    if (!p->armed) return 0;
    p->hits++;
    if (p->hits != p->nth && !(p->repeat && p->hits > p->nth)) return 0;
    switch (p->action) {
    case FP_ENOSPC: SetLastError(ERROR_HANDLE_DISK_FULL); return 1;
    case FP_EIO: SetLastError(ERROR_IO_DEVICE); return 1;
    default: die(name); return 1;
    }
}

int zs_fail_torn(void) {
    Point *p;
    if (!env_loaded) load_env();
    p = find("journal_torn", 0);
    if (!p || !p->armed) return 0;
    p->hits++;
    return p->hits == p->nth;
}
#endif
