/* Millennium-era chrome, drawn with plain GDI as utilities and media players
 * drew it around 1999-2001: a chrome wordmark over a perspective grid, a
 * glowing LCD readout with unlit ghost segments, a segmented level meter and
 * LED activity lights. Everything shown is a live reading of the running job;
 * nothing animates on its own. */
#ifndef ZS_Y2K_H
#define ZS_Y2K_H
#include "engine.h"

#define Y2K_BARS 28
enum { LED_COMPRESS, LED_VERIFY, LED_COMMIT, LED_DELETE };

typedef struct {
    u64 speeds[Y2K_BARS]; /* bytes per second, newest last */
    int n;
    u32 last_t;
    u64 last_b;
    u32 activity[4]; /* tick when each activity was last seen */
} Telemetry;

void tel_reset(Telemetry *t);
void tel_sample(Telemetry *t, const Stats *s);
void tel_activity(Telemetry *t, int led);

enum { Y2K_BANNER, Y2K_LCD, Y2K_METER, Y2K_LEDS };
/* Registers the "ZSY2K" control class; the control's id picks what it draws
 * (Y2K_BANNER + 9000 and so on). */
void y2k_register(void);
/* What the readouts show; the caller repaints them. */
void y2k_set(const Stats *s, const Telemetry *t, int running, const char *mode);
#define Y2K_ID(kind) (9000 + (kind))
#endif
