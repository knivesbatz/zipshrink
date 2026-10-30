/* Fault injection for the test suite.
 *
 * Built with -DZS_FAILPOINTS, named points can be armed with the
 * ZIPSHRINK_FAILPOINT environment variable as name[:N[+]][=action]
 * (comma separated): N is the hit that triggers (default 1; with '+', that
 * hit and every later one); action is abort (default: the process ends at
 * once, as in a crash or forced termination), enospc or eio (the operation
 * fails with that error). In-process tests arm points and hooks directly.
 * Release builds compile every point to nothing.
 */
#ifndef ZS_FAILPOINT_H
#define ZS_FAILPOINT_H
#include "base.h"

/* Exit code of a simulated crash, distinct from every exit code of the program. */
#define ZS_FP_EXIT 70

#ifdef ZS_FAILPOINTS
int zs_fail(const char *name); /* 1 when the operation must fail (LastError set) */
typedef void (*ZsHook)(void *ctx);
void zs_fp_arm(const char *name, u32 nth, int repeat, int action);
void zs_fp_hook(const char *name, ZsHook fn, void *ctx);
void zs_fp_reset(void);
enum { FP_ABORT, FP_ENOSPC, FP_EIO };
/* Writes the first half of a journal record, then ends the process. */
int zs_fail_torn(void);
#else
static inline int zs_fail(const char *name) {
    (void)name;
    return 0;
}
static inline int zs_fail_torn(void) { return 0; }
#endif
#endif
