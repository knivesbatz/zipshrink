#include "y2k.h"
#include "uw.h"

#define LCD_BG RGB(4, 20, 16)
#define LCD_ON RGB(80, 255, 190)
#define LCD_OFF RGB(14, 48, 38)
#define LCD_DIM RGB(40, 150, 112)

static Stats g_stats;
static Telemetry g_tel;
static int g_running;
static char g_mode[32] = "STANDBY";

void tel_reset(Telemetry *t) {
    memset(t, 0, sizeof(*t));
    t->last_t = zs_ticks();
}

void tel_sample(Telemetry *t, const Stats *s) {
    u32 now = zs_ticks(), dt = now - t->last_t;
    u64 bytes = s->phase == PH_VERIFY ? s->verify_done : s->bytes_processed, v;
    int first = !t->last_b;
    if (dt < 250) return;
    /* Compressing: the engine's smoothed rate (a resumed job's recovered
     * bytes are not throughput). Verifying: bytes read per interval. */
    v = s->phase == PH_COMPRESS ? s->speed : (bytes > t->last_b ? bytes - t->last_b : 0) * 1000 / dt;
    t->last_t = now, t->last_b = bytes ? bytes : 1;
    if (first && s->phase != PH_COMPRESS) return;
    if (t->n == Y2K_BARS) memmove(t->speeds, t->speeds + 1, sizeof(t->speeds[0]) * (Y2K_BARS - 1)), t->n--;
    t->speeds[t->n++] = v;
}

void tel_activity(Telemetry *t, int led) { t->activity[led] = zs_ticks() | 1; }

void y2k_set(const Stats *s, const Telemetry *t, int running, const char *mode) {
    size_t n = strlen(mode);
    if (s) g_stats = *s;
    if (t) g_tel = *t;
    g_running = running;
    if (n >= sizeof(g_mode)) n = sizeof(g_mode) - 1;
    memcpy(g_mode, mode, n);
    g_mode[n] = 0;
}

/* ---------- drawing helpers ---------- */

static HFONT font(int height, int weight, int italic, const char *face) {
    return CreateFontA(height, 0, 0, 0, weight, (DWORD)italic, 0, 0, ANSI_CHARSET, OUT_DEFAULT_PRECIS, CLIP_DEFAULT_PRECIS, NONANTIALIASED_QUALITY,
                       DEFAULT_PITCH | FF_SWISS, face);
}

static void fill(HDC dc, int x, int y, int w, int h, COLORREF c) {
    RECT r;
    HBRUSH b = CreateSolidBrush(c);
    SetRect(&r, x, y, x + w, y + h);
    FillRect(dc, &r, b);
    DeleteObject(b);
}

static void line(HDC dc, int x0, int y0, int x1, int y1, COLORREF c) {
    HPEN p = CreatePen(PS_SOLID, 1, c), old = SelectObject(dc, p);
    MoveToEx(dc, x0, y0, NULL);
    LineTo(dc, x1, y1);
    SelectObject(dc, old);
    DeleteObject(p);
}

static void text(HDC dc, int x, int y, const char *s, COLORREF c) {
    SetTextColor(dc, c);
    SetBkMode(dc, TRANSPARENT);
    TextOutA(dc, x, y, s, (int)strlen(s));
}

static int text_w(HDC dc, const char *s) {
    SIZE z;
    GetTextExtentPoint32A(dc, s, (int)strlen(s), &z);
    return z.cx;
}

static COLORREF mix(COLORREF a, COLORREF b, int num, int den) {
    return RGB(GetRValue(a) + (GetRValue(b) - GetRValue(a)) * num / den, GetGValue(a) + (GetGValue(b) - GetGValue(a)) * num / den,
               GetBValue(a) + (GetBValue(b) - GetBValue(a)) * num / den);
}

/* Sunken frame like a 1998 status panel. */
static void sunken(HDC dc, RECT *r) { DrawEdge(dc, r, EDGE_SUNKEN, BF_RECT | BF_ADJUST); }

/* ---------- banner ---------- */

static void banner(HDC dc, RECT rc) {
    int w = rc.right - rc.left, h = rc.bottom - rc.top, horizon = h * 58 / 100, i;
    u32 seed = 7;
    HFONT big, small, old;
    const char *mark = "ZipShrink";
    int tx = 16, ty = 4, tw, th;
    SIZE z;
    /* Sky. */
    for (i = 0; i < horizon; i++) fill(dc, 0, i, w, 1, mix(RGB(0, 0, 32), RGB(36, 8, 92), i, horizon));
    /* Stars. */
    for (i = 0; i < w / 6; i++) {
        int x, y;
        seed = seed * 1103515245 + 12345;
        x = (int)((seed >> 8) % (u32)w);
        seed = seed * 1103515245 + 12345;
        y = (int)((seed >> 8) % (u32)(horizon - 2));
        SetPixel(dc, x, y, (seed >> 20) & 1 ? RGB(255, 255, 255) : RGB(140, 140, 200));
    }
    /* Floor and perspective grid. */
    for (i = horizon; i < h; i++) fill(dc, 0, i, w, 1, mix(RGB(10, 0, 30), RGB(60, 0, 80), i - horizon, h - horizon));
    for (i = 1; i < 9; i++) {
        int y = horizon + (h - horizon) * i * i / 64;
        line(dc, 0, y, w, y, RGB(220, 40, 220));
    }
    for (i = -14; i <= 14; i++) {
        int bx = w / 2 + i * w / 10;
        line(dc, w / 2 + i * 6, horizon, bx, h, RGB(220, 40, 220));
    }
    line(dc, 0, horizon, w, horizon, RGB(255, 120, 255));
    /* Chrome wordmark: outline, then horizontal bands of light. */
    big = font(-(h * 52 / 100), FW_HEAVY, 1, "Arial");
    old = SelectObject(dc, big);
    GetTextExtentPoint32A(dc, mark, (int)strlen(mark), &z);
    tw = z.cx, th = z.cy;
    {
        /* Orbit swoosh behind the letters. */
        HPEN p = CreatePen(PS_SOLID, 2, RGB(255, 150, 30)), op = SelectObject(dc, p);
        HBRUSH ob = SelectObject(dc, GetStockObject(NULL_BRUSH));
        Arc(dc, tx - 10, ty + th / 6, tx + tw + 14, ty + th - 2, tx + tw + 14, ty + th / 3, tx - 10, ty + th / 2);
        SelectObject(dc, ob);
        SelectObject(dc, op);
        DeleteObject(p);
    }
    text(dc, tx + 2, ty + 2, mark, RGB(0, 0, 0));
    text(dc, tx - 1, ty, mark, RGB(20, 20, 70));
    text(dc, tx + 1, ty, mark, RGB(20, 20, 70));
    text(dc, tx, ty - 1, mark, RGB(20, 20, 70));
    text(dc, tx, ty + 1, mark, RGB(20, 20, 70));
    {
        static const COLORREF band[] = {RGB(255, 255, 255), RGB(220, 230, 255), RGB(160, 180, 230), RGB(60, 70, 150), RGB(130, 150, 220), RGB(230, 240, 255)};
        int n = (int)ARRAY_LEN(band), k;
        for (k = 0; k < n; k++) {
            int y0 = ty + th * k / n, y1 = ty + th * (k + 1) / n;
            int saved = SaveDC(dc);
            IntersectClipRect(dc, tx - 2, y0, tx + tw + 4, y1);
            text(dc, tx, ty, mark, band[k]);
            RestoreDC(dc, saved);
        }
    }
    /* Lens sparkle. */
    {
        int sx = tx + tw + 2, sy = ty + th / 5;
        line(dc, sx - 7, sy, sx + 8, sy, RGB(255, 255, 255));
        line(dc, sx, sy - 7, sx, sy + 8, RGB(255, 255, 255));
        line(dc, sx - 2, sy - 2, sx + 3, sy + 3, RGB(200, 220, 255));
        line(dc, sx - 2, sy + 2, sx + 3, sy - 3, RGB(200, 220, 255));
    }
    SelectObject(dc, old);
    DeleteObject(big);
    small = font(-11, FW_NORMAL, 0, "MS Sans Serif");
    old = SelectObject(dc, small);
    text(dc, tx + 12, h - 16, "Space-saving ZIP archiver  -  Millennium Edition", RGB(255, 255, 255));
    {
        char v[32];
        int vw;
        memcpy(v, "v" ZS_VERSION, sizeof("v" ZS_VERSION));
        vw = text_w(dc, v);
        text(dc, w - vw - 8, 6, v, RGB(160, 200, 255));
    }
    SelectObject(dc, old);
    DeleteObject(small);
}

/* ---------- LCD ---------- */

static const u8 SEG[10] = {0x3F, 0x06, 0x5B, 0x4F, 0x66, 0x6D, 0x7D, 0x07, 0x7F, 0x6F};

static void segment(HDC dc, int seg, int x, int y, int w, int h, int t, COLORREF c) {
    POINT p[6];
    int hh = h / 2, n = 6;
    HBRUSH b = CreateSolidBrush(c), ob = SelectObject(dc, b);
    HPEN op = SelectObject(dc, GetStockObject(NULL_PEN));
    switch (seg) {
    case 0: /* a */
    case 6: /* g */
    case 3: { /* d */
        int yy = seg == 0 ? y : seg == 6 ? y + hh - t / 2 : y + h - t;
        p[0].x = x + 1, p[0].y = yy + t / 2;
        p[1].x = x + 1 + t / 2, p[1].y = yy;
        p[2].x = x + w - 1 - t / 2, p[2].y = yy;
        p[3].x = x + w - 1, p[3].y = yy + t / 2;
        p[4].x = x + w - 1 - t / 2, p[4].y = yy + t;
        p[5].x = x + 1 + t / 2, p[5].y = yy + t;
        break;
    }
    default: {
        int xx = (seg == 1 || seg == 2) ? x + w - t : x;
        int y0 = (seg == 1 || seg == 5) ? y + 1 : y + hh + 1, y1 = (seg == 1 || seg == 5) ? y + hh - 1 : y + h - 1;
        p[0].x = xx + t / 2, p[0].y = y0;
        p[1].x = xx + t, p[1].y = y0 + t / 2;
        p[2].x = xx + t, p[2].y = y1 - t / 2;
        p[3].x = xx + t / 2, p[3].y = y1;
        p[4].x = xx, p[4].y = y1 - t / 2;
        p[5].x = xx, p[5].y = y0 + t / 2;
    }
    }
    Polygon(dc, p, n);
    SelectObject(dc, op);
    SelectObject(dc, ob);
    DeleteObject(b);
}

static void digit(HDC dc, int d, int x, int y, int w, int h) {
    int s, t = w / 5 < 3 ? 3 : w / 5;
    for (s = 0; s < 7; s++) segment(dc, s, x, y, w, h, t, LCD_OFF);
    if (d >= 0)
        for (s = 0; s < 7; s++)
            if (SEG[d] >> s & 1) segment(dc, s, x, y, w, h, t, LCD_ON);
}

static void lcd(HDC dc, RECT rc) {
    int w = rc.right - rc.left, h = rc.bottom - rc.top;
    u32 pm = g_running || g_stats.phase == PH_DONE ? stats_permille(&g_stats) : 0, pc = pm / 10;
    int dw = h * 30 / 100, dh = h * 56 / 100, x = 10, y = 9, k;
    HFONT f = font(-11, FW_BOLD, 0, "MS Sans Serif"), old;
    char *rate, *freed, files[48], line1[48];
    fill(dc, 0, 0, w, h, LCD_BG);
    digit(dc, pc >= 100 ? 1 : -1, x, y, dw, dh);
    digit(dc, pc >= 10 ? (int)(pc / 10 % 10) : -1, x + dw + 6, y, dw, dh);
    digit(dc, (int)(pc % 10), x + 2 * (dw + 6), y, dw, dh);
    old = SelectObject(dc, f);
    text(dc, x + 3 * (dw + 6) + 2, y, "%", LCD_ON);
    k = x + 3 * (dw + 6) + 26;
    rate = zs_human(g_stats.speed);
    freed = zs_human(g_stats.reclaimed);
    memcpy(files, "", 1);
    {
        char *t = zs_fmt("%U/%U", g_stats.files_archived, g_stats.total_files);
        size_t n = strlen(t) < sizeof(files) - 1 ? strlen(t) : sizeof(files) - 1;
        memcpy(files, t, n), files[n] = 0;
        zs_free(t);
    }
    text(dc, k, y, "RATE", LCD_DIM);
    text(dc, k, y + 16, "FREED", LCD_DIM);
    text(dc, k, y + 32, "FILES", LCD_DIM);
    {
        char *r2 = zs_join(rate, "/s");
        text(dc, k + 46, y, g_running ? r2 : "--", LCD_ON);
        zs_free(r2);
    }
    text(dc, k + 46, y + 16, freed, LCD_ON);
    text(dc, k + 46, y + 32, files, LCD_ON);
    {
        size_t n = strlen(g_mode);
        memcpy(line1, ">> ", 3);
        if (n > sizeof(line1) - 4) n = sizeof(line1) - 4;
        memcpy(line1 + 3, g_mode, n), line1[3 + n] = 0;
    }
    text(dc, x, h - 16, line1, LCD_DIM);
    SelectObject(dc, old);
    DeleteObject(f);
    zs_free(rate), zs_free(freed);
}

/* ---------- level meter ---------- */

static void meter(HDC dc, RECT rc) {
    int w = rc.right - rc.left, h = rc.bottom - rc.top, i, s;
    const int segs = 12, top = 20, bottom = h - 6;
    int colw = (w - 16) / Y2K_BARS;
    u64 peak = 1;
    HFONT f = font(-11, FW_NORMAL, 0, "MS Sans Serif"), old;
    fill(dc, 0, 0, w, h, RGB(0, 0, 0));
    for (i = 0; i < g_tel.n; i++)
        if (g_tel.speeds[i] > peak) peak = g_tel.speeds[i];
    old = SelectObject(dc, f);
    text(dc, 8, 4, "THROUGHPUT", RGB(90, 140, 255));
    if (g_tel.n) {
        char *p = zs_human(peak), *t = zs_fmt("PEAK %s/s", p);
        text(dc, w - 8 - text_w(dc, t), 4, t, RGB(90, 140, 255));
        zs_free(p), zs_free(t);
    }
    SelectObject(dc, old);
    DeleteObject(f);
    for (i = 0; i < Y2K_BARS; i++) {
        int idx = g_tel.n - Y2K_BARS + i, lit = 0;
        if (idx >= 0) lit = (int)((g_tel.speeds[idx] * (u64)segs + peak - 1) / peak);
        for (s = 0; s < segs; s++) {
            int sh = (bottom - top) / segs, y = bottom - (s + 1) * sh;
            COLORREF on = s >= segs - 2 ? RGB(240, 50, 40) : s >= segs * 6 / 10 ? RGB(240, 200, 40) : RGB(60, 220, 60);
            fill(dc, 8 + i * colw, y + 1, colw - 2, sh - 2, s < lit ? on : RGB(28, 28, 28));
        }
    }
}

/* ---------- LEDs ---------- */

static void leds(HDC dc, RECT rc) {
    static const char *names[] = {"COMPRESS", "VERIFY", "COMMIT", "DELETE"};
    static const COLORREF lit[] = {RGB(60, 240, 60), RGB(250, 220, 40), RGB(60, 220, 255), RGB(255, 50, 40)};
    int w = rc.right - rc.left, h = rc.bottom - rc.top, i;
    HFONT f = font(-11, FW_NORMAL, 0, "MS Sans Serif"), old = SelectObject(dc, f);
    u32 now = zs_ticks();
    fill(dc, 0, 0, w, h, GetSysColor(COLOR_BTNFACE));
    for (i = 0; i < 4; i++) {
        int y = 6 + i * (h - 12) / 4, on;
        Phase ph = g_stats.phase;
        on = g_running && ((g_tel.activity[i] && now - g_tel.activity[i] < 220) || (i == LED_VERIFY && ph == PH_VERIFY) || (i == LED_COMPRESS && ph == PH_COMPRESS && g_stats.has_current));
        {
            HBRUSH b = CreateSolidBrush(on ? lit[i] : mix(lit[i], RGB(0, 0, 0), 3, 4)), ob = SelectObject(dc, b);
            HPEN p = CreatePen(PS_SOLID, 1, RGB(64, 64, 64)), op = SelectObject(dc, p);
            Ellipse(dc, 8, y + 2, 18, y + 12);
            SelectObject(dc, op), SelectObject(dc, ob);
            DeleteObject(p), DeleteObject(b);
            if (on) SetPixel(dc, 11, y + 4, RGB(255, 255, 255));
        }
        text(dc, 26, y, names[i], GetSysColor(COLOR_BTNTEXT));
    }
    SelectObject(dc, old);
    DeleteObject(f);
}

static LRESULT CALLBACK y2k_proc(HWND hw, UINT m, WPARAM wp, LPARAM lp) {
    if (m == WM_PAINT) {
        PAINTSTRUCT ps;
        HDC dc = BeginPaint(hw, &ps), mem;
        RECT rc, in;
        HBITMAP bmp, ob;
        int kind = GetDlgCtrlID(hw) - 9000;
        GetClientRect(hw, &rc);
        in = rc;
        if (kind != Y2K_BANNER) sunken(dc, &in);
        mem = CreateCompatibleDC(dc);
        bmp = CreateCompatibleBitmap(dc, in.right - in.left, in.bottom - in.top);
        ob = SelectObject(mem, bmp);
        {
            RECT z;
            SetRect(&z, 0, 0, in.right - in.left, in.bottom - in.top);
            switch (kind) {
            case Y2K_BANNER: banner(mem, z); break;
            case Y2K_LCD: lcd(mem, z); break;
            case Y2K_METER: meter(mem, z); break;
            default: leds(mem, z); break;
            }
        }
        BitBlt(dc, in.left, in.top, in.right - in.left, in.bottom - in.top, mem, 0, 0, SRCCOPY);
        SelectObject(mem, ob);
        DeleteObject(bmp);
        DeleteDC(mem);
        EndPaint(hw, &ps);
        return 0;
    }
    if (m == WM_ERASEBKGND) return 1;
    return uw_defproc(hw, m, wp, lp);
}

void y2k_register(void) { uw_register(0, y2k_proc, NULL, NULL, NULL, 0, "ZSY2K"); }
