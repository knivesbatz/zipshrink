/* zipshrink-gui.exe: the windowed program, in the style of a 1998 Windows
 * utility (classic controls, MS Sans Serif) with millennium-era readouts. */
#include "engine.h"
#include "update.h"
#include "uw.h"
#include "y2k.h"
#include "../res/resource.h"

#define CW 776 /* client size: the window fits an 800 x 600 screen */
#define CH 520

enum {
    WM_PF_DONE = WM_APP + 1,
    WM_TICK,
    WM_RUN_DONE,
    WM_CHECK_DONE,
    WM_UPD_DONE,
    WM_DL_PROGRESS,
    WM_DL_DONE,
};

enum { P_SETUP = 1, P_RUN = 2, P_DONE = 4 };

enum {
    ID_STATUSBAR = 100,
    /* setup */
    ID_G_FILES,
    ID_SOURCES,
    ID_ADD_FILES,
    ID_ADD_FOLDER,
    ID_REMOVE,
    ID_CLEAR,
    ID_TIP,
    ID_G_ARCHIVE,
    ID_L_DIR,
    ID_DIR,
    ID_BROWSE,
    ID_L_NAME,
    ID_NAME,
    ID_L_LEVEL,
    ID_LEVEL,
    ID_L_PARTS,
    ID_SPLIT,
    ID_PARTS,
    ID_PARTS_UD,
    ID_PART_SIZE,
    ID_PARTS_NOTE,
    ID_L_FREE,
    ID_FREE,
    ID_FREE_NOTE,
    ID_G_ORIG,
    ID_DELETE,
    ID_DELETE_NOTE,
    ID_DELETE_RO,
    ID_G_SPACE,
    ID_L_SEL,
    ID_SEL,
    ID_L_EST,
    ID_EST,
    ID_L_NEED,
    ID_NEED,
    ID_L_AVAIL,
    ID_AVAIL,
    ID_SPACE_BAR,
    ID_VERDICT,
    ID_PARTS_TITLE,
    ID_PARTS_LIST,
    ID_GO,
    ID_CLOSE,
    /* run */
    ID_RUN_TITLE,
    ID_RUN_ARCHIVE,
    ID_RUN_FILE,
    ID_RUN_FILE_BAR,
    ID_RUN_OVERALL,
    ID_RUN_BAR,
    ID_G_DETAILS,
    ID_D_FIRST,                /* 9 label/value pairs */
    ID_D_LAST = ID_D_FIRST + 17,
    ID_TAB_ACTIVITY,
    ID_TAB_PROBLEMS,
    ID_ACTIVITY,
    ID_PROBLEMS,
    ID_PAUSE,
    /* done */
    ID_DONE_ICON,
    ID_DONE_TITLE,
    ID_DONE_TEXT,
    ID_G_SUMMARY,
    ID_S_FIRST,
    ID_S_LAST = ID_S_FIRST + 17,
    ID_OPEN_FOLDER,
    ID_VERIFY_AGAIN,
    ID_RESUME_JOB,
    ID_NEW,
    ID_DONE_PROBLEMS_TITLE,
    ID_DONE_PROBLEMS,
    ID_LAST
};

typedef struct {
    Status st;
    u64 size;
    char *name, *detail;
    int msg;
} Ev;

/* A scan + space check running on a worker thread. */
typedef struct {
    volatile LONG cancel;
    u32 gen;
    char **sources;
    size_t nsources;
    char *output;
    Options o;
    int parts;
    u64 part_size;
    /* results */
    int ok;
    char *error;
    char *out_abs;
    char **roots;
    size_t nroots;
    Scan scan;
    JobPaths paths;
    Preflight pf;
    Part *partv;
    int nparts;
} Pf;

typedef enum { RUN_JOBS, RUN_VERIFY } RunKind;

static struct {
    HWND w, sb;
    HWND c[ID_LAST];
    u8 page_of[ID_LAST];
    int page;
    HFONT font, bold, big;
    HBRUSH face;
    PtrVec sources; /* char* full paths */
    int name_touched, filling;
    /* space check */
    u32 gen;
    Pf *pending, *pf; /* running; latest finished for gen */
    int go_when_ready;
    /* the running job(s) */
    HANDLE thread;
    volatile LONG cancel;
    int running, closing;
    RunKind kind;
    Job **jobs;
    size_t njobs;
    Report **reports;
    size_t nreports;
    char *verify_path;
    ArchiveCheck vcheck;
    char *verror;
    int vok;
    u64 vtotal;
    CRITICAL_SECTION cs;
    Stats st;
    Telemetry tel;
    PtrVec evq;
    volatile LONG tick_posted;
    size_t part_index;
    u64 nproblems;
    char *last_output; /* for Verify Again / Open Folder */
    char *resume_path;
    int done_list;
    /* updates */
    int checking_update, manual_update;
    Release rel;
    char *dl_path, *dl_error;
} A;

/* ---------- small helpers ---------- */

static HWND C(int id) { return A.c[id]; }

static HWND ctl(int page, const char *cls, const char *text, DWORD style, DWORD ex, int x, int y, int w, int h, int id) {
    HWND h2 = uw_create(ex, cls, text, WS_CHILD | style, x, y, w, h, A.w, id);
    uw_send(h2, WM_SETFONT, (WPARAM)A.font, 0);
    if (id > 0 && id < ID_LAST) A.c[id] = h2, A.page_of[id] = (u8)page;
    return h2;
}

static HWND label(int page, const char *text, int x, int y, int w, int h, int id) { return ctl(page, "STATIC", text, SS_LEFT | SS_NOPREFIX, 0, x, y, w, h, id); }
static HWND button(int page, const char *text, int x, int y, int w, int h, int id) { return ctl(page, "BUTTON", text, BS_PUSHBUTTON | WS_TABSTOP, 0, x, y, w, h, id); }
static HWND group(int page, const char *text, int x, int y, int w, int h, int id) { return ctl(page, "BUTTON", text, BS_GROUPBOX, 0, x, y, w, h, id); }

static void set(int id, const char *s) { uw_settext(C(id), s); }
static void setf(int id, const char *fmt, ...) {
    Buf b = {0};
    va_list ap;
    va_start(ap, fmt);
    zs_vfmt(&b, fmt, ap);
    va_end(ap);
    set(id, buf_cstr(&b));
    buf_free(&b);
}

static char *get(int id) { return uw_gettext(C(id)); }
static int checked(int id) { return uw_send(C(id), BM_GETCHECK, 0, 0) == BST_CHECKED; }
static void enable(int id, int on) { EnableWindow(C(id), on); }
static void bold(int id) { uw_send(C(id), WM_SETFONT, (WPARAM)A.bold, 0); }

static char *human(u64 n) { return zs_human(n); }

static char *pct(u64 a, u64 b) {
    u64 t;
    if (!b) return zs_strdup("0");
    t = a > 0xFFFFFFFFFFFFULL ? a / (b / 1000 + 1) : a * 1000 / b;
    return zs_fmt("%U.%U", t / 10, t % 10);
}

static char *duration(u64 secs) {
    if (secs < 60) return zs_fmt("%U seconds", secs);
    if (secs < 3600) return zs_fmt("%U minute%s", (secs + 30) / 60, (secs + 30) / 60 == 1 ? "" : "s");
    return zs_fmt("%U h %U min", secs / 3600, secs % 3600 / 60);
}

static void msg(const char *text, UINT icon) { uw_msgbox(A.w, text, "ZipShrink", MB_OK | icon); }
static int ask(const char *text, UINT icon) { return uw_msgbox(A.w, text, "ZipShrink", MB_YESNO | icon) == IDYES; }

static void status_text(const char *s) { sb_text(A.sb, 0, s); }

static void show_free_space(const char *dir) {
    u64 f;
    if (dir && plat_free_space(dir, &f)) {
        char *h = human(f), *t = zs_fmt("%s free", h);
        sb_text(A.sb, 1, t);
        zs_free(h), zs_free(t);
    }
}

enum { SPLIT_NONE, SPLIT_COUNT, SPLIT_SIZE };

static int split_mode(void) {
    int m = (int)uw_send(C(ID_SPLIT), CB_GETCURSEL, 0, 0);
    return m == SPLIT_COUNT || m == SPLIT_SIZE ? m : SPLIT_NONE;
}

/* The count box or the size box, whichever the "Split" choice needs. */
static void split_controls(void) {
    int m = split_mode(), setup = A.page == P_SETUP;
    ShowWindow(C(ID_PARTS), setup && m == SPLIT_COUNT ? SW_SHOW : SW_HIDE);
    ShowWindow(C(ID_PARTS_UD), setup && m == SPLIT_COUNT ? SW_SHOW : SW_HIDE);
    ShowWindow(C(ID_PART_SIZE), setup && m == SPLIT_SIZE ? SW_SHOW : SW_HIDE);
    set(ID_PARTS_NOTE, m == SPLIT_COUNT ? "ZIP files" : m == SPLIT_SIZE ? "each" : "");
}

static void show_page(int page) {
    int id;
    A.page = page;
    for (id = 1; id < ID_LAST; id++)
        if (A.c[id] && A.page_of[id]) ShowWindow(A.c[id], (A.page_of[id] & page) ? SW_SHOW : SW_HIDE);
    for (id = Y2K_LCD; id <= Y2K_LEDS; id++) ShowWindow(GetDlgItem(A.w, Y2K_ID(id)), page == P_RUN ? SW_SHOW : SW_HIDE);
    split_controls();
    InvalidateRect(A.w, NULL, TRUE);
}

/* ---------- building the window ---------- */

static void build(void) {
    int y, i;
    HWND h;
    static const char *dl[] = {"Original data:", "Archive size:", "Space reclaimed:", "Free space:", "Compression:", "Speed:", "Originals deleted:", "Files done:", "Kept / failed:"};

    ctl(P_SETUP | P_RUN | P_DONE, "ZSY2K", "", WS_VISIBLE, 0, 0, 0, CW, 56, Y2K_ID(Y2K_BANNER));

    /* Setup page: files. */
    group(P_SETUP, "Files and folders to compress", 8, 60, 376, 246, ID_G_FILES);
    h = ctl(P_SETUP, WC_LISTVIEWA, "", LVS_REPORT | LVS_SHOWSELALWAYS | WS_TABSTOP | WS_BORDER, WS_EX_CLIENTEDGE, 16, 78, 268, 220, ID_SOURCES);
    lv_column(h, 0, "Type", 52, 0);
    lv_column(h, 1, "Location", 196, 0);
    button(P_SETUP, "Add &Files...", 292, 78, 84, 23, ID_ADD_FILES);
    button(P_SETUP, "Add F&older...", 292, 106, 84, 23, ID_ADD_FOLDER);
    button(P_SETUP, "&Remove", 292, 134, 84, 23, ID_REMOVE);
    button(P_SETUP, "Clear", 292, 162, 84, 23, ID_CLEAR);
    label(P_SETUP, "Tip: you can also drag files and folders onto this window.", 292, 196, 84, 80, ID_TIP);

    /* Archive. */
    group(P_SETUP, "Archive", 392, 60, 376, 150, ID_G_ARCHIVE);
    y = 78;
    label(P_SETUP, "Save in:", 402, y + 3, 66, 14, ID_L_DIR);
    ctl(P_SETUP, "EDIT", "", ES_AUTOHSCROLL | WS_TABSTOP, WS_EX_CLIENTEDGE, 470, y, 212, 21, ID_DIR);
    button(P_SETUP, "&Browse...", 688, y - 1, 72, 23, ID_BROWSE);
    y += 25;
    label(P_SETUP, "File name:", 402, y + 3, 66, 14, ID_L_NAME);
    ctl(P_SETUP, "EDIT", "", ES_AUTOHSCROLL | WS_TABSTOP, WS_EX_CLIENTEDGE, 470, y, 290, 21, ID_NAME);
    y += 25;
    label(P_SETUP, "Compression:", 402, y + 3, 66, 14, ID_L_LEVEL);
    h = ctl(P_SETUP, "COMBOBOX", "", CBS_DROPDOWNLIST | WS_VSCROLL | WS_TABSTOP, 0, 470, y, 150, 120, ID_LEVEL);
    cb_add(h, "Store (no compression)");
    cb_add(h, "Fastest");
    cb_add(h, "Normal");
    cb_add(h, "Maximum");
    uw_send(h, CB_SETCURSEL, 2, 0);
    y += 25;
    label(P_SETUP, "Split:", 402, y + 3, 66, 14, ID_L_PARTS);
    h = ctl(P_SETUP, "COMBOBOX", "", CBS_DROPDOWNLIST | WS_VSCROLL | WS_TABSTOP, 0, 470, y, 150, 120, ID_SPLIT);
    cb_add(h, "Don't split");
    cb_add(h, "Into a number of files");
    cb_add(h, "Into files of a size");
    uw_send(h, CB_SETCURSEL, SPLIT_NONE, 0);
    ctl(P_SETUP, "EDIT", "2", ES_NUMBER | WS_TABSTOP, WS_EX_CLIENTEDGE, 626, y, 52, 21, ID_PARTS);
    h = ctl(P_SETUP, UPDOWN_CLASSA, "", UDS_SETBUDDYINT | UDS_ALIGNRIGHT | UDS_ARROWKEYS | UDS_NOTHOUSANDS, 0, 0, 0, 0, 0, ID_PARTS_UD);
    uw_send(h, UDM_SETBUDDY, (WPARAM)C(ID_PARTS), 0);
    uw_send(h, UDM_SETRANGE, 0, MAKELPARAM(ZS_MAX_PARTS, 2));
    uw_send(h, UDM_SETPOS, 0, MAKELPARAM(2, 0));
    ctl(P_SETUP, "EDIT", "650 MB", ES_AUTOHSCROLL | WS_TABSTOP, WS_EX_CLIENTEDGE, 626, y, 70, 21, ID_PART_SIZE);
    label(P_SETUP, "", 702, y + 3, 58, 14, ID_PARTS_NOTE);
    y += 25;
    label(P_SETUP, "Keep free:", 402, y + 3, 66, 14, ID_L_FREE);
    ctl(P_SETUP, "EDIT", "256 MB", ES_AUTOHSCROLL | WS_TABSTOP, WS_EX_CLIENTEDGE, 470, y, 70, 21, ID_FREE);
    label(P_SETUP, "on the destination drive at all times", 546, y + 3, 214, 14, ID_FREE_NOTE);

    /* Original files. */
    group(P_SETUP, "Original files", 392, 214, 376, 92, ID_G_ORIG);
    ctl(P_SETUP, "BUTTON", "&Delete original files after successful compression", BS_AUTOCHECKBOX | WS_TABSTOP, 0, 402, 232, 356, 16, ID_DELETE);
    label(P_SETUP, "", 402, 252, 358, 28, ID_DELETE_NOTE);
    ctl(P_SETUP, "BUTTON", "Also delete read-&only files", BS_AUTOCHECKBOX | WS_TABSTOP, 0, 402, 283, 356, 16, ID_DELETE_RO);

    /* Disk space. */
    group(P_SETUP, "Disk space", 8, 310, 760, 150, ID_G_SPACE);
    {
        static const char *names[] = {"Selected:", "Estimated archive size:", "Space needed:", "Free on destination:"};
        for (i = 0; i < 4; i++) {
            label(P_SETUP, names[i], 18, 328 + i * 18, 130, 14, ID_L_SEL + 2 * i);
            label(P_SETUP, "", 150, 328 + i * 18, 230, 14, ID_SEL + 2 * i);
        }
    }
    h = ctl(P_SETUP, PROGRESS_CLASSA, "", 0, 0, 18, 404, 362, 14, ID_SPACE_BAR);
    uw_send(h, PBM_SETRANGE, 0, MAKELPARAM(0, 1000));
    label(P_SETUP, "Add files or folders to see whether the archive will fit.", 18, 420, 362, 39, ID_VERDICT);
    label(P_SETUP, "", 392, 326, 368, 14, ID_PARTS_TITLE);
    bold(ID_PARTS_TITLE);
    h = ctl(P_SETUP, WC_LISTVIEWA, "", LVS_REPORT | LVS_NOSORTHEADER | WS_BORDER | WS_TABSTOP, WS_EX_CLIENTEDGE, 392, 342, 368, 110, ID_PARTS_LIST);
    lv_column(h, 0, "Name", 112, 0);
    lv_column(h, 1, "Files", 42, 1);
    lv_column(h, 2, "Data", 60, 1);
    lv_column(h, 3, "Estimated", 64, 1);
    lv_column(h, 4, "At most", 60, 1);

    button(P_SETUP, "Compress", 574, 467, 104, 25, ID_GO);
    button(P_SETUP, "Close", 684, 467, 84, 25, ID_CLOSE);

    /* Progress page. */
    ctl(P_RUN, "ZSY2K", "", 0, 0, 8, 60, 282, 84, Y2K_ID(Y2K_LCD));
    ctl(P_RUN, "ZSY2K", "", 0, 0, 296, 60, 304, 84, Y2K_ID(Y2K_METER));
    ctl(P_RUN, "ZSY2K", "", 0, 0, 606, 60, 162, 84, Y2K_ID(Y2K_LEDS));
    label(P_RUN, "", 8, 150, 760, 14, ID_RUN_TITLE);
    bold(ID_RUN_TITLE);
    label(P_RUN, "", 8, 167, 760, 14, ID_RUN_ARCHIVE);
    label(P_RUN, "", 8, 184, 760, 14, ID_RUN_FILE);
    h = ctl(P_RUN, PROGRESS_CLASSA, "", 0, 0, 8, 201, 760, 16, ID_RUN_FILE_BAR);
    uw_send(h, PBM_SETRANGE, 0, MAKELPARAM(0, 1000));
    label(P_RUN, "", 8, 222, 760, 14, ID_RUN_OVERALL);
    h = ctl(P_RUN, PROGRESS_CLASSA, "", 0, 0, 8, 239, 760, 16, ID_RUN_BAR);
    uw_send(h, PBM_SETRANGE, 0, MAKELPARAM(0, 1000));
    group(P_RUN, "Details", 8, 260, 760, 84, ID_G_DETAILS);
    for (i = 0; i < 9; i++) {
        int col = i / 3, row = i % 3;
        label(P_RUN, dl[i], 18 + col * 252, 278 + row * 20, 104, 14, ID_D_FIRST + 2 * i);
        label(P_RUN, "", 124 + col * 252, 278 + row * 20, 140, 14, ID_D_FIRST + 2 * i + 1);
        bold(ID_D_FIRST + 2 * i + 1);
    }
    ctl(P_RUN, "BUTTON", "Activity", BS_AUTORADIOBUTTON | BS_PUSHLIKE | WS_TABSTOP | WS_GROUP, 0, 8, 350, 90, 22, ID_TAB_ACTIVITY);
    ctl(P_RUN, "BUTTON", "Problems (0)", BS_AUTORADIOBUTTON | BS_PUSHLIKE, 0, 102, 350, 110, 22, ID_TAB_PROBLEMS);
    uw_send(C(ID_TAB_ACTIVITY), BM_SETCHECK, BST_CHECKED, 0);
    h = ctl(P_RUN, WC_LISTVIEWA, "", LVS_REPORT | LVS_NOSORTHEADER | WS_BORDER, WS_EX_CLIENTEDGE, 8, 374, 760, 88, ID_ACTIVITY);
    lv_column(h, 0, "Status", 150, 0);
    lv_column(h, 1, "Size", 76, 1);
    lv_column(h, 2, "Name", 510, 0);
    h = ctl(P_RUN, WC_LISTVIEWA, "", LVS_REPORT | LVS_NOSORTHEADER | WS_BORDER, WS_EX_CLIENTEDGE, 8, 374, 760, 88, ID_PROBLEMS);
    lv_column(h, 0, "Name", 300, 0);
    lv_column(h, 1, "What happened", 436, 0);
    button(P_RUN, "&Pause", 684, 467, 84, 25, ID_PAUSE);

    /* Results page. */
    ctl(P_DONE, "STATIC", "", SS_ICON, 0, 16, 66, 32, 32, ID_DONE_ICON);
    label(P_DONE, "", 60, 68, 700, 16, ID_DONE_TITLE);
    bold(ID_DONE_TITLE);
    label(P_DONE, "", 60, 86, 700, 28, ID_DONE_TEXT);
    group(P_DONE, "Summary", 8, 118, 760, 196, ID_G_SUMMARY);
    for (i = 0; i < 9; i++) {
        label(P_DONE, "", 18, 136 + i * 19, 150, 14, ID_S_FIRST + 2 * i);
        label(P_DONE, "", 170, 136 + i * 19, 590, 14, ID_S_FIRST + 2 * i + 1);
    }
    button(P_DONE, "Open &Folder", 8, 322, 96, 25, ID_OPEN_FOLDER);
    button(P_DONE, "&Verify Again", 110, 322, 96, 25, ID_VERIFY_AGAIN);
    button(P_DONE, "&New Archive", 212, 322, 96, 25, ID_NEW);
    button(P_DONE, "&Resume", 314, 322, 96, 25, ID_RESUME_JOB);
    label(P_DONE, "", 8, 356, 760, 14, ID_DONE_PROBLEMS_TITLE);
    bold(ID_DONE_PROBLEMS_TITLE);
    h = ctl(P_DONE, WC_LISTVIEWA, "", LVS_REPORT | LVS_NOSORTHEADER | WS_BORDER, WS_EX_CLIENTEDGE, 8, 374, 760, 116, ID_DONE_PROBLEMS);
    lv_column(h, 0, "Name", 300, 0);
    lv_column(h, 1, "What happened", 436, 0);

    A.sb = uw_create(0, STATUSCLASSNAMEA, "Ready", WS_CHILD | WS_VISIBLE | SBARS_SIZEGRIP, 0, 0, 0, 0, A.w, ID_STATUSBAR);
    {
        int parts[3] = {CW - 260, CW - 130, -1};
        uw_send(A.sb, SB_SETPARTS, 3, (LPARAM)parts);
        sb_text(A.sb, 2, "Version " ZS_VERSION);
    }
    for (i = 1; i < ID_LAST; i++)
        if (A.c[i]) uw_send(A.c[i], WM_SETFONT, (WPARAM)(i == ID_RUN_TITLE || i == ID_DONE_TITLE || i == ID_PARTS_TITLE || (i >= ID_D_FIRST && i <= ID_D_LAST && (i - ID_D_FIRST) % 2) ? A.bold : A.font), 0);
}

/* ---------- setup page logic ---------- */

static void refresh_sources(void) {
    HWND lv = C(ID_SOURCES);
    size_t i;
    uw_send(lv, LVM_DELETEALLITEMS, 0, 0);
    for (i = 0; i < A.sources.len; i++) {
        Kind k;
        FStat st;
        const char *p = A.sources.v[i];
        int row = lv_insert(lv, (int)i, plat_kind(p, &k, &st) && k == K_DIR ? "Folder" : "File", (LPARAM)i);
        lv_set(lv, row, 1, p);
    }
    enable(ID_REMOVE, 0);
    enable(ID_CLEAR, A.sources.len > 0);
}

static void schedule_check(void) {
    if (A.filling) return;
    SetTimer(A.w, 1, 350, NULL);
}

static void add_sources(PtrVec *paths) {
    size_t i, k;
    int first = A.sources.len == 0;
    for (i = 0; i < paths->len; i++) {
        char *p = plat_full_path(paths->v[i]);
        int dup = 0;
        if (!p) continue;
        for (k = 0; k < A.sources.len; k++)
            if (!zs_stricmp(A.sources.v[k], p)) dup = 1;
        if (dup) zs_free(p);
        else pv_push(&A.sources, p);
    }
    if (first && A.sources.len) {
        const char *s = A.sources.v[0];
        char *dir = zs_dirname(s), *name;
        const char *base = zs_basename(s);
        A.filling = 1;
        {
            char *cur = get(ID_DIR);
            if (!*cur) set(ID_DIR, dir);
            zs_free(cur);
        }
        if (!A.name_touched) {
            const char *dot = NULL, *q;
            Kind k;
            FStat st;
            int is_dir = plat_kind(s, &k, &st) && k == K_DIR;
            for (q = base; *q; q++)
                if (*q == '.') dot = q;
            name = !is_dir && dot && dot != base ? zs_fmt("%s.zip", zs_strndup(base, (size_t)(dot - base))) : zs_fmt("%s.zip", base);
            set(ID_NAME, *base ? name : "Archive.zip");
            zs_free(name);
        }
        A.filling = 0;
        zs_free(dir);
    }
    refresh_sources();
    schedule_check();
}

static int level_of(int sel) {
    static const int levels[] = {0, 1, 6, 9};
    return sel >= 0 && sel < 4 ? levels[sel] : 6;
}

static int parts_value(void) {
    char *t;
    u64 v = 2;
    if (split_mode() != SPLIT_COUNT) return 1;
    t = get(ID_PARTS);
    if (!zs_parse_u64(t, &v) || v < 2) v = 2;
    if (v > ZS_MAX_PARTS) v = ZS_MAX_PARTS;
    zs_free(t);
    return (int)v;
}

/* The size of each part in "Into files of a size" mode, 0 otherwise or
 * (with *why) when the box does not hold a usable size. */
static u64 part_size_value(const char **why) {
    char *t;
    u64 v = 0;
    if (split_mode() != SPLIT_SIZE) return 0;
    t = get(ID_PART_SIZE);
    if (!zs_parse_size(t, &v) || v < ZS_MIN_PART_SIZE) *why = "The size of each ZIP file should be a size such as 650 MB or 2 GB (at least 64 KB).", v = 0;
    zs_free(t);
    return v;
}

/* The output path from the two boxes; NULL with *why when unusable. */
static char *output_path(const char **why) {
    char *dir = get(ID_DIR), *name = get(ID_NAME), *out = NULL;
    *why = NULL;
    if (!*dir) *why = "Choose the folder to save the archive in.";
    else if (!*name) *why = "Type a name for the archive.";
    else {
        char *n2 = zs_ends_with_i(name, ".zip") ? zs_strdup(name) : zs_join(name, ".zip");
        out = zs_path_join(dir, n2);
        zs_free(n2);
    }
    zs_free(dir), zs_free(name);
    return out;
}

static int read_options(Options *o, const char **why) {
    char *f = get(ID_FREE);
    u64 v;
    options_default(o);
    o->level = level_of((int)uw_send(C(ID_LEVEL), CB_GETCURSEL, 0, 0));
    o->delete_originals = checked(ID_DELETE);
    o->delete_readonly = o->delete_originals && checked(ID_DELETE_RO);
    *why = NULL;
    if (!zs_parse_size(f, &v)) *why = "\"Keep free\" should be a size such as 256 MB or 2 GB.";
    else o->min_free = v;
    zs_free(f);
    return *why == NULL;
}

static void pf_free(Pf *p) {
    size_t i;
    if (!p) return;
    for (i = 0; i < p->nsources; i++) zs_free(p->sources[i]);
    zs_free(p->sources);
    zs_free(p->output), zs_free(p->error), zs_free(p->out_abs);
    for (i = 0; i < p->nroots; i++) zs_free(p->roots[i]);
    zs_free(p->roots);
    scan_free(&p->scan);
    paths_free(&p->paths);
    preflight_free(&p->pf);
    if (p->partv) parts_free(p->partv, p->nparts);
    zs_free(p);
}

static DWORD WINAPI pf_thread(void *arg) {
    Pf *p = arg;
    char *e = NULL;
    const char *excl[3];
    if (!job_validate((const char *const *)p->sources, p->nsources, p->output, &p->out_abs, &p->roots, &p->nroots, &e)) goto out;
    paths_for_output(&p->paths, p->out_abs);
    excl[0] = p->paths.output, excl[1] = p->paths.partial, excl[2] = p->paths.journal;
    if (!scan_build(&p->scan, p->roots, p->nroots, excl, 3, &p->cancel, &e)) goto out;
    if (!preflight(&p->scan, &p->paths, &p->o, &p->pf, &e)) goto out;
    if (p->parts > 1) {
        if (!split_scan(&p->scan, p->paths.output, p->parts, p->pf.ratio_permille, &p->partv, &e)) goto out;
        p->nparts = p->parts;
    } else if (p->part_size) {
        if (!split_scan_size(&p->scan, p->paths.output, p->part_size, p->pf.ratio_permille, &p->partv, &p->nparts, &e)) goto out;
    }
    p->ok = 1;
out:
    p->error = e;
    uw_post(A.w, WM_PF_DONE, 0, (LPARAM)p);
    return 0;
}

static void clear_space(const char *verdict) {
    int i;
    for (i = 0; i < 4; i++) set(ID_SEL + 2 * i, "");
    uw_send(C(ID_SPACE_BAR), PBM_SETPOS, 0, 0);
    set(ID_VERDICT, verdict);
    set(ID_PARTS_TITLE, "");
    uw_send(C(ID_PARTS_LIST), LVM_DELETEALLITEMS, 0, 0);
    ShowWindow(C(ID_PARTS_LIST), SW_HIDE);
}

static int verdict_color;

static void start_check(void) {
    Pf *p;
    const char *why = NULL;
    char *out;
    Options o;
    size_t i;
    KillTimer(A.w, 1);
    A.gen++;
    if (A.pending) A.pending->cancel = 1, A.pending = NULL;
    pf_free(A.pf), A.pf = NULL;
    split_controls();
    {
        int del = checked(ID_DELETE);
        enable(ID_DELETE_RO, del);
        set(ID_DELETE_NOTE, del ? "Each original is PERMANENTLY deleted - not sent to the Recycle Bin - once its compressed copy is verified on disk."
                                : "Your files are left untouched. Turn this on to free disk space while the archive is built.");
        set(ID_GO, del ? "Compress && Delete" : "Compress");
        InvalidateRect(C(ID_DELETE_NOTE), NULL, TRUE);
    }
    if (!A.sources.len) {
        verdict_color = 0;
        clear_space("Add files or folders to see whether the archive will fit.");
        return;
    }
    out = output_path(&why);
    if (!out || !read_options(&o, &why) || (part_size_value(&why), why)) {
        verdict_color = 2;
        clear_space(why);
        zs_free(out);
        return;
    }
    p = zs_zalloc(sizeof(Pf));
    p->gen = A.gen;
    p->sources = zs_alloc(A.sources.len * sizeof(char *));
    for (i = 0; i < A.sources.len; i++) p->sources[i] = zs_strdup(A.sources.v[i]);
    p->nsources = A.sources.len;
    p->output = out;
    p->o = o;
    p->parts = parts_value();
    p->part_size = part_size_value(&why);
    A.pending = p;
    verdict_color = 0;
    set(ID_VERDICT, "Checking the selection and the free space...");
    {
        DWORD tid;
        HANDLE t = CreateThread(NULL, 0, pf_thread, p, 0, &tid);
        if (t) CloseHandle(t);
        else pf_thread(p);
    }
}

static void show_check(Pf *p) {
    Preflight *f = &p->pf;
    char *a, *b, *c;
    int i;
    if (!p->ok) {
        verdict_color = 2;
        clear_space(p->error ? p->error : "The selection cannot be archived.");
        return;
    }
    a = human(f->bytes), b = human(f->estimated), c = human(f->worst);
    setf(ID_SEL, "%s in %U files and %U folders", a, f->files, f->dirs);
    setf(ID_EST, "about %s (at most %s)", b, c);
    zs_free(a), zs_free(b), zs_free(c);
    a = human(f->need_estimate), b = human(f->need_worst);
    setf(ID_NEED, f->delete_originals ? "about %s (originals are deleted as it goes)" : "about %s (at most %s)", a, b);
    zs_free(a), zs_free(b);
    a = human(f->free_space), b = human(f->min_free);
    setf(ID_AVAIL, "%s (%s kept free)", a, b);
    zs_free(a), zs_free(b);
    {
        u64 avail = f->free_space > f->min_free ? f->free_space - f->min_free : 0;
        u32 bar = avail ? (u32)(f->need_worst >= avail ? 1000 : f->need_worst * 1000 / avail) : 1000;
        uw_send(C(ID_SPACE_BAR), PBM_SETPOS, bar, 0);
    }
    verdict_color = f->verdict == V_FITS ? 1 : f->verdict == V_PROBABLY ? 3 : 2;
    {
        Buf t = {0};
        buf_puts(&t, f->explanation);
        if (p->scan.nskips) buf_fmt(&t, " %U items cannot be archived and will be left alone.", (u64)p->scan.nskips);
        if (p->part_size && p->partv) {
            int over = 0;
            for (i = 0; i < p->nparts; i++)
                if (p->partv[i].files == 1 && p->partv[i].estimated > p->part_size) over++;
            if (over == 1) buf_puts(&t, " One file is larger than the size chosen, so it gets a larger ZIP file of its own.");
            else if (over) buf_fmt(&t, " %d files are larger than the size chosen, so each gets a larger ZIP file of its own.", over);
        }
        set(ID_VERDICT, buf_cstr(&t));
        buf_free(&t);
    }
    show_free_space(f->dest_dir);
    uw_send(C(ID_PARTS_LIST), LVM_DELETEALLITEMS, 0, 0);
    if (p->nparts > 1) {
        if (p->part_size) {
            char *t = human(p->part_size);
            setf(ID_PARTS_TITLE, "Split into %d ZIP files of about %s or less:", p->nparts, t);
            zs_free(t);
        } else
            setf(ID_PARTS_TITLE, "The archive will be split into %d ZIP files:", p->nparts);
        for (i = 0; i < p->nparts; i++) {
            Part *q = &p->partv[i];
            char *s1 = zs_fmt("%U", q->files), *s2 = human(q->bytes), *s3 = human(q->estimated), *s4 = human(q->worst);
            int row = lv_insert(C(ID_PARTS_LIST), i, zs_basename(q->output), 0);
            lv_set(C(ID_PARTS_LIST), row, 1, s1);
            lv_set(C(ID_PARTS_LIST), row, 2, s2);
            lv_set(C(ID_PARTS_LIST), row, 3, s3);
            lv_set(C(ID_PARTS_LIST), row, 4, s4);
            zs_free(s1), zs_free(s2), zs_free(s3), zs_free(s4);
        }
        ShowWindow(C(ID_PARTS_LIST), A.page == P_SETUP ? SW_SHOW : SW_HIDE);
    } else if (p->part_size) {
        char *t = human(p->part_size);
        setf(ID_PARTS_TITLE, "Everything fits in one ZIP file of %s.", t);
        zs_free(t);
        ShowWindow(C(ID_PARTS_LIST), SW_HIDE);
    } else {
        set(ID_PARTS_TITLE, "Tip: \"Split\" makes several separate, complete ZIP files.");
        ShowWindow(C(ID_PARTS_LIST), SW_HIDE);
    }
}

/* ---------- the consent dialog ---------- */

static INT_PTR CALLBACK confirm_proc(HWND d, UINT m, WPARAM wp, LPARAM lp) {
    switch (m) {
    case WM_INITDIALOG:
        uw_send(GetDlgItem(d, IDC_WARNICON), STM_SETICON, (WPARAM)LoadIconA(NULL, (const char *)IDI_EXCLAMATION), 0);
        uw_setitemtext(d, IDC_TEXT, (const char *)lp);
        {
            LOGFONTA lf;
            HFONT f = (HFONT)SendMessageA(d, WM_GETFONT, 0, 0);
            static HFONT bf;
            if (!bf && GetObjectA(f, sizeof(lf), &lf)) lf.lfWeight = FW_BOLD, bf = CreateFontIndirectA(&lf);
            if (bf) SendMessageA(GetDlgItem(d, IDC_TITLE), WM_SETFONT, (WPARAM)bf, 0);
        }
        return TRUE;
    case WM_COMMAND:
        if (LOWORD(wp) == IDC_UNDERSTAND) EnableWindow(GetDlgItem(d, IDOK), IsDlgButtonChecked(d, IDC_UNDERSTAND) == BST_CHECKED);
        if (LOWORD(wp) == IDOK || LOWORD(wp) == IDCANCEL) EndDialog(d, LOWORD(wp));
        return TRUE;
    }
    return FALSE;
}

static int confirm_delete(void) {
    const char *text = "ZipShrink will delete each original permanently - it is NOT sent to the Recycle Bin - as soon as its compressed copy has been written, "
                       "flushed to the disk, read back, decompressed, matched against the original's CRC-32 and SHA-256, and recorded in the recovery journal.\r\n\r\n"
                       "Files that change, cannot be verified or are read-only are kept. After a crash the job can be resumed, but the archive will then hold the only "
                       "copy of the deleted files. Keep a backup of anything irreplaceable.";
    return uw_dialog(IDD_CONFIRM, A.w, confirm_proc, (LPARAM)text) == IDOK;
}

/* ---------- running jobs ---------- */

static void ev_push(Ev *e) {
    EnterCriticalSection(&A.cs);
    pv_push(&A.evq, e);
    LeaveCriticalSection(&A.cs);
}

static void post_tick(void) {
    if (!A.tick_posted) {
        A.tick_posted = 1;
        uw_post(A.w, WM_TICK, 0, 0);
    }
}

static void ob_phase(void *ctx, Phase p) {
    EnterCriticalSection(&A.cs);
    A.st.phase = p;
    LeaveCriticalSection(&A.cs);
    post_tick();
}

static void ob_entry(void *ctx, u64 idx, const char *name, u64 size, Status st, const char *detail) {
    int led = st == ST_COMPRESSING ? LED_COMPRESS : st == ST_VERIFYING ? LED_VERIFY : st == ST_ARCHIVED ? LED_COMMIT : st == ST_DELETED ? LED_DELETE : -1;
    EnterCriticalSection(&A.cs);
    if (led >= 0) tel_activity(&A.tel, led);
    LeaveCriticalSection(&A.cs);
    if (st != ST_COMPRESSING && st != ST_VERIFYING && st != ST_WAITING) {
        Ev *e = zs_zalloc(sizeof(Ev));
        e->st = st, e->size = size, e->name = zs_strdup(name), e->detail = detail ? zs_strdup(detail) : NULL;
        ev_push(e);
    }
    post_tick();
}

static void ob_progress(void *ctx, const Stats *s) {
    EnterCriticalSection(&A.cs);
    A.st = *s;
    LeaveCriticalSection(&A.cs);
    post_tick();
}

static void ob_message(void *ctx, const char *text) {
    Ev *e = zs_zalloc(sizeof(Ev));
    e->msg = 1, e->name = zs_strdup(text);
    ev_push(e);
    post_tick();
}

static DWORD WINAPI run_thread(void *arg) {
    size_t i;
    Observer o;
    (void)arg;
    o.ctx = NULL, o.phase = ob_phase, o.entry = ob_entry, o.progress = ob_progress, o.message = ob_message;
    if (A.kind == RUN_VERIFY) {
        char *e = NULL;
        A.vok = check_archive(A.verify_path, 1, &A.cancel, NULL, NULL, &A.vcheck, &e);
        A.verror = e;
    } else {
        A.reports = zs_zalloc(A.njobs * sizeof(Report *));
        for (i = 0; i < A.njobs; i++) {
            Report *r;
            A.part_index = i;
            if (A.njobs > 1) {
                char *t = zs_fmt("Part %U of %U: %s", (u64)(i + 1), (u64)A.njobs, job_output(A.jobs[i]));
                ob_message(NULL, t);
                zs_free(t);
            }
            r = job_run(A.jobs[i], &o, &A.cancel);
            A.reports[A.nreports++] = r;
            if (r->outcome == OUT_STOPPED) break;
        }
    }
    uw_post(A.w, WM_RUN_DONE, 0, 0);
    return 0;
}

static void verify_bytes(void *ctx, u64 n) {
    (void)ctx;
    EnterCriticalSection(&A.cs);
    A.st.verify_done += n;
    LeaveCriticalSection(&A.cs);
    post_tick();
}

static void start_thread(void) {
    DWORD tid;
    A.running = 1;
    A.cancel = 0;
    A.tick_posted = 0;
    A.nproblems = 0;
    memset(&A.st, 0, sizeof(A.st));
    tel_reset(&A.tel);
    uw_send(C(ID_ACTIVITY), LVM_DELETEALLITEMS, 0, 0);
    uw_send(C(ID_PROBLEMS), LVM_DELETEALLITEMS, 0, 0);
    set(ID_TAB_PROBLEMS, "Problems (0)");
    uw_send(C(ID_TAB_ACTIVITY), BM_SETCHECK, BST_CHECKED, 0);
    uw_send(C(ID_TAB_PROBLEMS), BM_SETCHECK, BST_UNCHECKED, 0);
    set(ID_PAUSE, A.kind == RUN_VERIFY ? "&Stop" : "&Pause");
    enable(ID_PAUSE, 1);
    show_page(P_RUN);
    ShowWindow(C(ID_PROBLEMS), SW_HIDE);
    EnableMenuItem(GetMenu(A.w), IDM_NEW, MF_GRAYED);
    EnableMenuItem(GetMenu(A.w), IDM_RESUME, MF_GRAYED);
    EnableMenuItem(GetMenu(A.w), IDM_VERIFY, MF_GRAYED);
    EnableMenuItem(GetMenu(A.w), IDM_SALVAGE, MF_GRAYED);
    y2k_set(&A.st, &A.tel, 1, A.kind == RUN_VERIFY ? "VERIFY" : "ONLINE");
    SetTimer(A.w, 2, 100, NULL);
    A.thread = CreateThread(NULL, 0, run_thread, NULL, 0, &tid);
    if (!A.thread) {
        A.running = 0;
        msg("ZipShrink could not start its worker thread.", MB_ICONSTOP);
        show_page(P_SETUP);
    }
}

static void free_jobs(void) {
    size_t i;
    for (i = 0; i < A.njobs; i++) job_free(A.jobs[i]);
    zs_free(A.jobs);
    A.jobs = NULL, A.njobs = 0;
}

static void free_reports(void) {
    size_t i;
    for (i = 0; i < A.nreports; i++) report_free(A.reports[i]);
    zs_free(A.reports);
    A.reports = NULL, A.nreports = 0;
}

static void add_problem(HWND lv, const char *name, const char *what) {
    int row = lv_insert(lv, (int)uw_send(lv, LVM_GETITEMCOUNT, 0, 0), name, 0);
    lv_set(lv, row, 1, what);
}

static void on_tick(void) {
    Stats s;
    PtrVec evs = {0};
    size_t i;
    A.tick_posted = 0;
    EnterCriticalSection(&A.cs);
    s = A.st;
    evs = A.evq;
    memset(&A.evq, 0, sizeof(A.evq));
    if (A.kind == RUN_VERIFY) s.phase = PH_VERIFY, s.verify_total = A.vtotal;
    tel_sample(&A.tel, &s);
    y2k_set(&s, &A.tel, 1, A.kind == RUN_VERIFY ? "VERIFY" : s.phase == PH_COMPRESS ? "ONLINE" : s.phase == PH_VERIFY ? "VERIFY" : "STANDBY");
    LeaveCriticalSection(&A.cs);
    for (i = 0; i < evs.len; i++) {
        Ev *e = evs.v[i];
        if (e->msg) {
            add_problem(C(ID_PROBLEMS), "", e->name);
            status_text(e->name);
        } else {
            HWND lv = C(ID_ACTIVITY);
            char *sz = e->size ? human(e->size) : zs_strdup("");
            int row = lv_insert(lv, 0, status_label(e->st), (LPARAM)e->st);
            lv_set(lv, row, 1, sz);
            lv_set(lv, row, 2, e->name);
            zs_free(sz);
            while (uw_send(lv, LVM_GETITEMCOUNT, 0, 0) > 500) uw_send(lv, LVM_DELETEITEM, 500, 0);
            if (e->st == ST_FAILED || e->st == ST_KEPT || e->st == ST_SKIPPED) {
                add_problem(C(ID_PROBLEMS), e->name, e->detail ? e->detail : status_label(e->st));
                A.nproblems++;
                setf(ID_TAB_PROBLEMS, "Problems (%U)", A.nproblems);
            }
        }
        zs_free(e->name), zs_free(e->detail), zs_free(e);
    }
    pv_free(&evs, 0);
    {
        u32 pm = stats_permille(&s);
        char *t = zs_fmt("%u%% - ZipShrink", pm / 10);
        uw_settext(A.w, t);
        zs_free(t);
        if (A.kind == RUN_VERIFY) {
            char *a = human(s.verify_done), *b = human(A.vtotal);
            set(ID_RUN_TITLE, "Verifying...");
            setf(ID_RUN_ARCHIVE, "Archive: %s", A.verify_path);
            set(ID_RUN_FILE, "Every entry is decompressed and checked by CRC-32, and by SHA-256 where ZipShrink stored one.");
            setf(ID_RUN_OVERALL, "Overall: %u%%  (%s of %s)", pm / 10, a, b);
            zs_free(a), zs_free(b);
            uw_send(C(ID_RUN_FILE_BAR), PBM_SETPOS, pm, 0);
        } else {
            const char *out = A.part_index < A.njobs ? job_output(A.jobs[A.part_index]) : "";
            char *a = human(s.bytes_processed), *b = human(s.total_bytes);
            if (A.njobs > 1) setf(ID_RUN_TITLE, "%s... (part %U of %U)", phase_label(s.phase), (u64)(A.part_index + 1), (u64)A.njobs);
            else setf(ID_RUN_TITLE, "%s...", phase_label(s.phase));
            setf(ID_RUN_ARCHIVE, "Archive: %s", out);
            if (s.has_current && s.phase == PH_COMPRESS) {
                char *c = human(s.cur_done), *d = human(s.cur_size);
                setf(ID_RUN_FILE, "File: %s   (%s of %s)", s.cur_name, c, d);
                zs_free(c), zs_free(d);
                uw_send(C(ID_RUN_FILE_BAR), PBM_SETPOS, s.cur_size ? (WPARAM)(s.cur_done * 1000 / s.cur_size) : 0, 0);
            } else if (s.phase == PH_VERIFY && s.verify_total) {
                set(ID_RUN_FILE, "Reading the finished archive back and checking every entry");
                uw_send(C(ID_RUN_FILE_BAR), PBM_SETPOS, (WPARAM)(s.verify_done * 1000 / s.verify_total), 0);
            } else {
                set(ID_RUN_FILE, "");
                uw_send(C(ID_RUN_FILE_BAR), PBM_SETPOS, 0, 0);
            }
            if (s.phase == PH_VERIFY && s.verify_total) {
                char *c = human(s.verify_done), *d = human(s.verify_total);
                setf(ID_RUN_OVERALL, "Verification: %u%%  (%s of %s read back and checked)", pm / 10, c, d);
                zs_free(c), zs_free(d);
            } else if (s.speed && s.total_bytes > s.bytes_processed && s.phase == PH_COMPRESS) {
                char *eta = duration((s.total_bytes - s.bytes_processed) / s.speed);
                setf(ID_RUN_OVERALL, "Overall: %u%%  (%s of %s)   %s remaining", pm / 10, a, b, eta);
                zs_free(eta);
            } else {
                setf(ID_RUN_OVERALL, "Overall: %u%%  (%s of %s)", pm / 10, a, b);
            }
            zs_free(a), zs_free(b);
        }
        uw_send(C(ID_RUN_BAR), PBM_SETPOS, pm, 0);
    }
    if (A.kind != RUN_VERIFY) {
        char *v[9];
        v[0] = human(s.total_bytes);
        v[1] = human(s.archive_size);
        v[2] = human(s.reclaimed);
        v[3] = human(s.free_space);
        v[4] = s.bytes_committed ? zs_fmt("%s%% of original", pct(s.compressed_committed, s.bytes_committed)) : zs_strdup("-");
        {
            char *sp = human(s.speed);
            v[5] = zs_fmt("%s/s", sp);
            zs_free(sp);
        }
        v[6] = s.delete_originals ? zs_fmt("%U", s.deleted) : zs_strdup("off");
        v[7] = zs_fmt("%U (%U remaining)", s.files_archived, stats_remaining_files(&s));
        v[8] = zs_fmt("%U / %U", s.kept, s.failed);
        for (i = 0; i < 9; i++) set(ID_D_FIRST + 2 * (int)i + 1, v[i]), zs_free(v[i]);
        {
            char *f = human(s.free_space), *t = zs_fmt("%s free", f);
            sb_text(A.sb, 1, t);
            zs_free(f), zs_free(t);
        }
        status_text(phase_label(s.phase));
    } else {
        for (i = 0; i < 9; i++) set(ID_D_FIRST + 2 * (int)i + 1, "");
        status_text("Verifying");
    }
    for (i = Y2K_LCD; i <= Y2K_LEDS; i++) InvalidateRect(GetDlgItem(A.w, Y2K_ID((int)i)), NULL, FALSE);
}

static void summary_row(int i, const char *name, const char *value) {
    set(ID_S_FIRST + 2 * i, name);
    set(ID_S_FIRST + 2 * i + 1, value);
}

static void done_icon(LPCSTR which) { uw_send(C(ID_DONE_ICON), STM_SETICON, (WPARAM)LoadIconA(NULL, which), 0); }

static void show_verify_result(void) {
    HWND lv = C(ID_DONE_PROBLEMS);
    size_t i;
    int ok = A.vok && archive_ok(&A.vcheck);
    for (i = 0; i < 9; i++) summary_row((int)i, "", "");
    uw_send(lv, LVM_DELETEALLITEMS, 0, 0);
    done_icon(ok ? (LPCSTR)IDI_ASTERISK : (LPCSTR)IDI_HAND);
    set(ID_DONE_TITLE, ok ? "The archive passed verification." : A.cancel ? "Verification was stopped." : "The archive FAILED verification.");
    set(ID_DONE_TEXT, A.verror ? A.verror : "");
    summary_row(0, "Archive:", A.verify_path);
    if (A.vok) {
        char *b = human(A.vcheck.bytes), *t = zs_fmt("%U (%U files, %U folders), %s of data", A.vcheck.entries, A.vcheck.files, A.vcheck.dirs, b);
        summary_row(1, "Entries:", t);
        zs_free(b), zs_free(t);
        t = zs_fmt("%U files matched their stored SHA-256 and CRC-32", A.vcheck.sha_checked);
        summary_row(2, "Checked:", t);
        zs_free(t);
        if (A.vcheck.crc_only) {
            t = zs_fmt("%U files have no stored SHA-256 (not made by ZipShrink) and matched their CRC-32", A.vcheck.crc_only);
            summary_row(3, "", t);
            zs_free(t);
        }
    }
    for (i = 0; i < A.vcheck.problems.len; i++) add_problem(lv, "", A.vcheck.problems.v[i]);
    set(ID_DONE_PROBLEMS_TITLE, A.vcheck.problems.len ? "Problems found:" : "");
    A.done_list = A.vcheck.problems.len > 0;
    zs_free(A.last_output);
    A.last_output = zs_strdup(A.verify_path);
    ShowWindow(C(ID_RESUME_JOB), SW_HIDE);
}

static void show_report(void) {
    HWND lv = C(ID_DONE_PROBLEMS);
    size_t i, k;
    u64 orig = 0, arch = 0, recl = 0, files = 0, dirs = 0, deleted = 0, dirs_removed = 0, nkept = 0, nfailed = 0, nskipped = 0, entries = 0, checked_files = 0;
    int stopped = 0, resumable = 0, all_verified = 1, any_fail = 0, delete_req = 0;
    Report *last = A.nreports ? A.reports[A.nreports - 1] : NULL;
    char *t;
    uw_send(lv, LVM_DELETEALLITEMS, 0, 0);
    for (i = 0; i < 9; i++) summary_row((int)i, "", "");
    for (i = 0; i < A.nreports; i++) {
        Report *r = A.reports[i];
        orig += r->original_bytes, arch += r->archive_size, recl += r->reclaimed, files += r->files_archived, dirs += r->dirs_archived;
        deleted += r->deleted, dirs_removed += r->dirs_removed, nkept += r->nkept, nfailed += r->nfailed, nskipped += r->nskipped;
        delete_req |= r->delete_requested;
        if (r->outcome == OUT_STOPPED) stopped = 1, resumable = r->resumable;
        if (r->verified) {
            entries += r->check.entries, checked_files += r->check.sha_checked + r->check.crc_only;
            if (!archive_ok(&r->check)) any_fail = 1;
        } else if (r->outcome != OUT_STOPPED) {
            all_verified = 0;
        }
        for (k = 0; k < r->nfailed; k++) add_problem(lv, r->failed[k].path, r->failed[k].reason);
        for (k = 0; k < r->nkept; k++) add_problem(lv, r->kept[k].path, r->kept[k].reason);
        for (k = 0; k < r->nskipped; k++) add_problem(lv, r->skipped[k].path, r->skipped[k].reason);
        for (k = 0; k < r->warnings.len; k++) add_problem(lv, "", r->warnings.v[k]);
    }
    zs_free(A.resume_path), A.resume_path = NULL;
    if (stopped && last) {
        done_icon(resumable ? (LPCSTR)IDI_EXCLAMATION : (LPCSTR)IDI_HAND);
        set(ID_DONE_TITLE, last->stop == STOP_CANCELLED ? "The job is paused." : resumable ? "The job stopped safely and can be resumed." : "The job stopped.");
        if (resumable) {
            t = zs_fmt("%s  Everything archived so far (%U entries) is safely stored in %s. Click Resume to continue, now or later.", last->stop == STOP_CANCELLED ? "" : last->error,
                       last->recoverable, last->paths.partial);
            A.resume_path = zs_strdup(last->paths.partial);
        } else {
            t = zs_fmt("%s  No further originals will be deleted. Keep %s and its journal; File > Salvage can rebuild a readable archive from what is intact.",
                       last->error ? last->error : "", last->paths.partial);
        }
        set(ID_DONE_TEXT, t);
        zs_free(t);
    } else {
        done_icon(nkept || nfailed || nskipped ? (LPCSTR)IDI_EXCLAMATION : (LPCSTR)IDI_ASTERISK);
        set(ID_DONE_TITLE, A.nreports > 1 ? "The archives were created successfully." : "The archive was created successfully.");
        set(ID_DONE_TEXT, nkept || nfailed || nskipped ? "Some files were left in place; they are listed below." : "");
    }
    if (A.nreports == 1 && last) summary_row(0, "Archive location:", last->paths.output);
    else if (last) {
        char *dir = zs_dirname(last->paths.output);
        t = zs_fmt("%U ZIP files in %s", (u64)A.nreports, dir);
        summary_row(0, "Archive location:", t);
        zs_free(t), zs_free(dir);
    }
    t = human(orig);
    summary_row(1, "Total original size:", t);
    zs_free(t);
    {
        char *h = human(arch), *p = pct(arch, orig);
        t = orig ? zs_fmt("%s  (%s%% of original)", h, p) : zs_strdup(h);
        summary_row(2, stopped ? "Archive size so far:" : "Final archive size:", t);
        zs_free(h), zs_free(p), zs_free(t);
    }
    t = human(recl);
    summary_row(3, "Disk space reclaimed:", delete_req ? t : "none (originals were kept)");
    zs_free(t);
    t = zs_fmt("%U files, %U folders", files, dirs);
    summary_row(4, "Files archived:", t);
    zs_free(t);
    t = delete_req ? zs_fmt("%U files, %U emptied folders removed", deleted, dirs_removed) : zs_strdup("untouched");
    summary_row(5, "Originals deleted:", t);
    zs_free(t);
    t = zs_fmt("%U skipped, %U failed, %U originals kept", nskipped, nfailed, nkept);
    summary_row(6, "Skipped or failed:", t);
    zs_free(t);
    if (!stopped) {
        if (any_fail) t = zs_strdup("FAILED - see the problems below");
        else if (all_verified) t = zs_fmt("Passed - every entry read back; %U files matched SHA-256 and CRC-32", checked_files);
        else t = zs_strdup("Each entry was verified as it was written");
        summary_row(7, "Verification:", t);
        zs_free(t);
    }
    set(ID_DONE_PROBLEMS_TITLE, uw_send(lv, LVM_GETITEMCOUNT, 0, 0) ? "Files left in place, and notes:" : "");
    A.done_list = uw_send(lv, LVM_GETITEMCOUNT, 0, 0) > 0;
    zs_free(A.last_output);
    A.last_output = last ? zs_strdup(stopped ? last->paths.partial : last->paths.output) : NULL;
    ShowWindow(C(ID_RESUME_JOB), A.resume_path ? SW_SHOW : SW_HIDE);
    (void)entries;
}

static void on_run_done(void) {
    WaitForSingleObject(A.thread, INFINITE);
    CloseHandle(A.thread);
    A.thread = NULL;
    KillTimer(A.w, 2);
    on_tick();
    A.running = 0;
    y2k_set(NULL, NULL, 0, "STANDBY");
    EnableMenuItem(GetMenu(A.w), IDM_NEW, MF_ENABLED);
    EnableMenuItem(GetMenu(A.w), IDM_RESUME, MF_ENABLED);
    EnableMenuItem(GetMenu(A.w), IDM_VERIFY, MF_ENABLED);
    EnableMenuItem(GetMenu(A.w), IDM_SALVAGE, MF_ENABLED);
    if (A.closing) {
        DestroyWindow(A.w);
        return;
    }
    uw_settext(A.w, "ZipShrink");
    if (A.kind == RUN_VERIFY) {
        show_verify_result();
        archive_check_free(&A.vcheck);
        zs_free(A.verror), A.verror = NULL;
    } else {
        show_report();
        free_jobs();
        free_reports();
    }
    status_text("Done");
    show_page(P_DONE);
    ShowWindow(C(ID_RESUME_JOB), A.resume_path ? SW_SHOW : SW_HIDE);
    ShowWindow(C(ID_VERIFY_AGAIN), A.last_output && !A.resume_path ? SW_SHOW : SW_HIDE);
    ShowWindow(C(ID_DONE_PROBLEMS), A.done_list ? SW_SHOW : SW_HIDE);
    MessageBeep(MB_OK);
}

/* ---------- starting work ---------- */

static void go(void) {
    Pf *p = A.pf;
    Options o;
    const char *why;
    char *e = NULL;
    ErrKind k;
    if (!A.sources.len) return msg("Add the files and folders to compress first.", MB_ICONINFORMATION);
    if (!p || p->gen != A.gen) {
        A.go_when_ready = 1;
        if (!A.pending) start_check();
        status_text("Checking the selection...");
        return;
    }
    A.go_when_ready = 0;
    if (!p->ok) return msg(p->error ? p->error : "The selection cannot be archived.", MB_ICONSTOP);
    if (!read_options(&o, &why)) return msg(why, MB_ICONSTOP);
    o.overwrite = 0;
    if (paths_unfinished(&p->paths)) {
        if (ask("An unfinished job for this archive already exists. Resume it instead?", MB_ICONQUESTION)) {
            extern void resume_path(const char *path);
            resume_path(p->paths.partial);
        }
        return;
    }
    if (p->nparts <= 1 && plat_exists(p->paths.output)) {
        char *t = zs_fmt("%s already exists.\r\n\r\nReplace it? The old file is replaced only once the new archive has been verified.", p->paths.output);
        int yes = ask(t, MB_ICONQUESTION);
        zs_free(t);
        if (!yes) return;
        o.overwrite = 1;
    }
    if (p->pf.verdict == V_NO && !ask("There may not be enough disk space for this archive.\r\n\r\nStart anyway? If space runs out, the job pauses safely and can be resumed later.", MB_ICONEXCLAMATION))
        return;
    if (o.delete_originals && !confirm_delete()) return;
    A.kind = RUN_JOBS;
    if (p->nparts > 1) {
        if (!jobs_create_parts(&o, p->roots, p->nroots, p->partv, p->nparts, &A.jobs, &e, &k)) goto fail;
        A.njobs = (size_t)p->nparts;
    } else {
        Job *j = job_create(&o, p->roots, p->nroots, &p->scan, p->out_abs, &e, &k);
        if (!j) goto fail;
        A.jobs = zs_alloc(sizeof(Job *));
        A.jobs[0] = j;
        A.njobs = 1;
    }
    pf_free(A.pf), A.pf = NULL;
    A.gen++;
    start_thread();
    return;
fail:
    msg(e, MB_ICONSTOP);
    zs_free(e);
    pf_free(A.pf), A.pf = NULL;
    A.gen++;
    start_check();
}

void resume_path(const char *path) {
    PtrVec t = resume_targets(path);
    size_t i;
    int any_delete = 0;
    if (!t.len) {
        char *m = zs_fmt("No unfinished job was found for %s.", path);
        msg(m, MB_ICONINFORMATION);
        zs_free(m);
        return;
    }
    A.jobs = zs_zalloc(t.len * sizeof(Job *));
    A.njobs = 0;
    for (i = 0; i < t.len; i++) {
        char *e = NULL;
        ErrKind k;
        Job *j = job_open(t.v[i], &e, &k);
        if (!j) {
            char *m = k == ERR_BUSY ? zs_fmt("%s\r\n\r\nThe job is open in another ZipShrink window or program.", e)
                                    : k == ERR_DAMAGED ? zs_fmt("%s\r\n\r\nNo originals will be deleted. File > Salvage can rebuild a readable archive from what is intact.", e)
                                                       : zs_strdup(e);
            msg(m, MB_ICONSTOP);
            zs_free(m), zs_free(e);
            free_jobs();
            pv_free(&t, 1);
            return;
        }
        A.jobs[A.njobs++] = j;
        if (job_options(j)->delete_originals) any_delete = 1;
    }
    pv_free(&t, 1);
    if (any_delete) {
        int r = uw_msgbox(A.w,
                          "This job was started with \"Delete original files\" turned on.\r\n\r\n"
                          "Yes: continue, deleting each original once its archived copy is verified.\r\nNo: finish the archive but delete no more originals.",
                          "Resume - ZipShrink", MB_YESNOCANCEL | MB_ICONQUESTION);
        if (r == IDCANCEL) return free_jobs();
        if (r == IDYES && !confirm_delete()) return free_jobs();
        if (r == IDNO)
            for (i = 0; i < A.njobs; i++) job_disable_delete(A.jobs[i]);
    }
    A.kind = RUN_JOBS;
    start_thread();
}

static void verify_path(const char *path) {
    HANDLE h = plat_open_read(path, 0);
    if (h == INVALID_HANDLE_VALUE) {
        char *m = zs_fmt("%s: %s", path, plat_error(GetLastError()));
        msg(m, MB_ICONSTOP);
        zs_free(m);
        return;
    }
    A.vtotal = plat_file_len(h);
    CloseHandle(h);
    zs_free(A.verify_path);
    A.verify_path = zs_strdup(path);
    A.kind = RUN_VERIFY;
    memset(&A.vcheck, 0, sizeof(A.vcheck));
    start_thread();
    (void)verify_bytes;
}

static void do_salvage(void) {
    PtrVec v = {0};
    SalvageReport r;
    char *e = NULL, *out;
    ErrKind k;
    if (!uw_open_files(A.w, "Salvage a partial archive", "Partial archives (*.partial)|*.partial|All files (*.*)|*.*", 0, &v)) return;
    out = salvage_default_output(v.v[0]);
    {
        char *q = zs_fmt("ZipShrink will check every entry in %s, keep each one that is complete and passes its checks, and turn the file into a "
                         "readable archive named:\r\n\r\n%s\r\n\r\nThis works in place, so it needs no extra disk space: an unfinished entry at the end "
                         "(which holds no complete file) is removed. Use Resume instead if the job's journal is intact. Continue?",
                         (char *)v.v[0], out);
        int yes = ask(q, MB_ICONQUESTION);
        zs_free(q);
        if (!yes) return zs_free(out), pv_free(&v, 1);
    }
    SetCursor(LoadCursorA(NULL, (const char *)IDC_WAIT));
    if (salvage(v.v[0], out, NULL, &r, &e, &k)) {
        char *b = human(r.bytes), *m = zs_fmt("Recovered %U entries (%U files, %U folders, %s) into\r\n%s\r\n\r\nEvery recovered file matched its CRC-32 (and SHA-256 where stored).",
                                                r.entries, r.files, r.dirs, b, r.output);
        msg(m, MB_ICONINFORMATION);
        zs_free(b), zs_free(m), zs_free(r.output);
        archive_check_free(&r.check);
    } else {
        msg(e, MB_ICONSTOP);
        zs_free(e);
    }
    zs_free(out);
    pv_free(&v, 1);
}

static void new_archive(void) {
    show_page(P_SETUP);
    uw_settext(A.w, "ZipShrink");
    status_text("Ready");
    schedule_check();
}

/* ---------- updates ---------- */

static DWORD WINAPI update_thread(void *arg) {
    int newer = 0;
    char *e = NULL;
    Release *r = zs_zalloc(sizeof(Release));
    (void)arg;
    if (!update_check(r, &newer, &e)) {
        release_free(r), zs_free(r);
        uw_post(A.w, WM_UPD_DONE, 0, (LPARAM)e);
        return 0;
    }
    uw_post(A.w, WM_UPD_DONE, (WPARAM)(newer ? 2 : 1), (LPARAM)r);
    return 0;
}

static void check_updates(int manual) {
    DWORD tid;
    HANDLE t;
    if (A.checking_update) return;
    A.checking_update = 1, A.manual_update = manual;
    if (manual) status_text("Checking for updates...");
    t = CreateThread(NULL, 0, update_thread, NULL, 0, &tid);
    if (t) CloseHandle(t);
    else A.checking_update = 0;
}

static void dl_progress(void *ctx, u64 done, u64 total) {
    (void)ctx;
    uw_post(A.w, WM_DL_PROGRESS, (WPARAM)(done >> 10), (LPARAM)(total >> 10));
}

static DWORD WINAPI download_thread(void *arg) {
    char *path = NULL, *e = NULL;
    (void)arg;
    if (update_download(&A.rel, &path, dl_progress, NULL, &e)) A.dl_path = path;
    else A.dl_error = e;
    uw_post(A.w, WM_DL_DONE, 0, 0);
    return 0;
}

static void on_update_result(WPARAM kind, LPARAM lp) {
    A.checking_update = 0;
    if (kind == 0) {
        char *e = (char *)lp;
        if (A.manual_update) {
            msg(e, MB_ICONEXCLAMATION);
            status_text("Ready");
        }
        zs_free(e);
        return;
    }
    if (!A.manual_update) uw_set_setting("LastUpdateCheck", (DWORD)zs_unix_now());
    {
        Release *r = (Release *)lp;
        if (kind == 1) {
            if (A.manual_update) {
                msg("You have the latest version of ZipShrink.", MB_ICONINFORMATION);
                status_text("Ready");
            }
            release_free(r), zs_free(r);
            return;
        }
        release_free(&A.rel);
        A.rel = *r;
        zs_free(r);
    }
    if (!A.manual_update) {
        char *t = zs_fmt("ZipShrink %s is available - Tools > Check for Updates", A.rel.version);
        status_text(t);
        zs_free(t);
        return;
    }
    if (!update_can_install()) {
        char *t = zs_fmt("ZipShrink %s is available.\r\n\r\nThis is a portable copy, so download the new version from the release page. Open it now?", A.rel.version);
        if (ask(t, MB_ICONQUESTION)) update_open_url(A.rel.page);
        zs_free(t);
        return;
    }
    {
        char *t = zs_fmt("ZipShrink %s is available (you have %s).\r\n\r\nDownload it, check its SHA-256 against the published checksums, and run the installer now?", A.rel.version,
                         ZS_VERSION);
        int yes = ask(t, MB_ICONQUESTION);
        zs_free(t);
        if (!yes) return;
    }
    if (A.running) return msg("Wait for the current job to finish (or pause it) before updating.", MB_ICONINFORMATION);
    {
        DWORD tid;
        HANDLE th = CreateThread(NULL, 0, download_thread, NULL, 0, &tid);
        if (th) CloseHandle(th);
        status_text("Downloading the update...");
    }
}

static void on_download_done(void) {
    char *e = NULL;
    if (!A.dl_path) {
        msg(A.dl_error ? A.dl_error : "The download failed.", MB_ICONSTOP);
        zs_free(A.dl_error), A.dl_error = NULL;
        status_text("Ready");
        return;
    }
    msg("The installer was downloaded and its SHA-256 matches the published checksum. ZipShrink will now close and start it.", MB_ICONINFORMATION);
    if (update_launch(A.dl_path, &e)) DestroyWindow(A.w);
    else msg(e, MB_ICONSTOP), zs_free(e);
}

static void about(void) {
    char *t = zs_fmt("ZipShrink %s\r\nCreate ZIP archives that reclaim disk space as they go.\r\n\r\nRunning on %s%s.\r\n\r\n"
                     "Free software under the MIT License.\r\nDeflate by miniz (MIT License).\r\nhttps://github.com/" UPDATE_REPO,
                     ZS_VERSION, zs_os_name(), uw_unicode() ? "" : " (file names in the system code page)");
    uw_msgbox(A.w, t, "About ZipShrink", MB_OK | MB_ICONINFORMATION);
    zs_free(t);
}

/* ---------- window procedure ---------- */

static void on_command(int id, int code) {
    switch (id) {
    case ID_ADD_FILES: {
        PtrVec v = {0};
        if (uw_open_files(A.w, "Add files", "All files (*.*)|*.*", 1, &v)) add_sources(&v);
        pv_free(&v, 1);
        break;
    }
    case ID_ADD_FOLDER: {
        char *d = uw_pick_folder(A.w, "Choose a folder to compress:");
        if (d) {
            PtrVec v = {0};
            pv_push(&v, d);
            add_sources(&v);
            pv_free(&v, 1);
        }
        break;
    }
    case ID_REMOVE: {
        HWND lv = C(ID_SOURCES);
        int i;
        for (i = (int)A.sources.len - 1; i >= 0; i--)
            if (uw_send(lv, LVM_GETITEMSTATE, (WPARAM)i, LVIS_SELECTED) & LVIS_SELECTED) {
                zs_free(A.sources.v[i]);
                memmove(&A.sources.v[i], &A.sources.v[i + 1], (A.sources.len - (size_t)i - 1) * sizeof(void *));
                A.sources.len--;
            }
        refresh_sources();
        schedule_check();
        break;
    }
    case ID_CLEAR:
        pv_free(&A.sources, 1);
        refresh_sources();
        schedule_check();
        break;
    case ID_BROWSE: {
        char *d = uw_pick_folder(A.w, "Save the archive in:");
        if (d) set(ID_DIR, d), zs_free(d);
        break;
    }
    case ID_NAME:
        if (code == EN_CHANGE && !A.filling) A.name_touched = 1;
        if (code == EN_CHANGE) schedule_check();
        break;
    case ID_DIR:
    case ID_FREE:
    case ID_PARTS:
    case ID_PART_SIZE:
        if (code == EN_CHANGE) schedule_check();
        break;
    case ID_SPLIT:
        if (code == CBN_SELCHANGE) split_controls(), schedule_check();
        break;
    case ID_LEVEL:
        if (code == CBN_SELCHANGE) schedule_check();
        break;
    case ID_DELETE:
    case ID_DELETE_RO: schedule_check(); break;
    case ID_GO: go(); break;
    case ID_CLOSE:
    case IDM_EXIT: SendMessageA(A.w, WM_CLOSE, 0, 0); break;
    case ID_PAUSE:
        if (A.running) {
            A.cancel = 1;
            enable(ID_PAUSE, 0);
            status_text("Stopping at the next safe point...");
        }
        break;
    case ID_TAB_ACTIVITY:
    case ID_TAB_PROBLEMS:
        ShowWindow(C(ID_ACTIVITY), id == ID_TAB_ACTIVITY ? SW_SHOW : SW_HIDE);
        ShowWindow(C(ID_PROBLEMS), id == ID_TAB_PROBLEMS ? SW_SHOW : SW_HIDE);
        break;
    case ID_OPEN_FOLDER:
        if (A.last_output) {
            char *d = zs_dirname(A.last_output);
            uw_shell_open(d);
            zs_free(d);
        }
        break;
    case ID_VERIFY_AGAIN:
        if (A.last_output) {
            char *p = zs_strdup(A.last_output);
            verify_path(p);
            zs_free(p);
        }
        break;
    case ID_RESUME_JOB:
        if (A.resume_path) {
            char *p = zs_strdup(A.resume_path);
            resume_path(p);
            zs_free(p);
        }
        break;
    case ID_NEW:
    case IDM_NEW:
        if (!A.running) new_archive();
        break;
    case IDM_RESUME: {
        PtrVec v = {0};
        if (!A.running && uw_open_files(A.w, "Resume an unfinished job", "Unfinished jobs (*.partial;*.journal)|*.partial;*.journal|All files (*.*)|*.*", 0, &v))
            resume_path(v.v[0]);
        pv_free(&v, 1);
        break;
    }
    case IDM_VERIFY: {
        PtrVec v = {0};
        if (!A.running && uw_open_files(A.w, "Verify an archive", "ZIP archives (*.zip)|*.zip|All files (*.*)|*.*", 0, &v)) verify_path(v.v[0]);
        pv_free(&v, 1);
        break;
    }
    case IDM_SALVAGE:
        if (!A.running) do_salvage();
        break;
    case IDM_UPDATE: check_updates(1); break;
    case IDM_AUTOUPDATE: {
        DWORD on = !uw_setting("CheckForUpdates", 1);
        uw_set_setting("CheckForUpdates", on);
        CheckMenuItem(GetMenu(A.w), IDM_AUTOUPDATE, on ? MF_CHECKED : MF_UNCHECKED);
        break;
    }
    case IDM_ABOUT: about(); break;
    }
}

static LRESULT CALLBACK wnd_proc(HWND hw, UINT m, WPARAM wp, LPARAM lp) {
    switch (m) {
    case WM_COMMAND: on_command(LOWORD(wp), HIWORD(wp)); return 0;
    case WM_NOTIFY: {
        NMHDR *n = (NMHDR *)lp;
        if (n->idFrom == ID_SOURCES && n->code == LVN_ITEMCHANGED) enable(ID_REMOVE, uw_send(C(ID_SOURCES), LVM_GETSELECTEDCOUNT, 0, 0) > 0);
        if (n->idFrom == ID_ACTIVITY && n->code == NM_CUSTOMDRAW) {
            NMLVCUSTOMDRAW *cd = (NMLVCUSTOMDRAW *)lp;
            if (cd->nmcd.dwDrawStage == CDDS_PREPAINT) return CDRF_NOTIFYITEMDRAW;
            if (cd->nmcd.dwDrawStage == CDDS_ITEMPREPAINT) {
                Status st = (Status)cd->nmcd.lItemlParam;
                cd->clrText = st == ST_DELETED ? RGB(0, 128, 0) : st == ST_ARCHIVED ? RGB(0, 128, 128) : st == ST_FAILED ? RGB(192, 0, 0) : st == ST_KEPT || st == ST_SKIPPED ? RGB(160, 96, 0) : RGB(0, 0, 160);
                return CDRF_DODEFAULT;
            }
        }
        return 0;
    }
    case WM_CTLCOLORSTATIC: {
        HDC dc = (HDC)wp;
        int id = GetDlgCtrlID((HWND)lp);
        SetBkColor(dc, GetSysColor(COLOR_BTNFACE));
        if (id == ID_VERDICT) SetTextColor(dc, verdict_color == 1 ? RGB(0, 128, 0) : verdict_color == 2 ? RGB(192, 0, 0) : verdict_color == 3 ? RGB(160, 96, 0) : GetSysColor(COLOR_BTNTEXT));
        else if (id == ID_DELETE_NOTE) SetTextColor(dc, checked(ID_DELETE) ? RGB(192, 0, 0) : GetSysColor(COLOR_GRAYTEXT));
        else SetTextColor(dc, GetSysColor(COLOR_BTNTEXT));
        return (LRESULT)A.face;
    }
    case WM_TIMER:
        if (wp == 1) start_check();
        if (wp == 2 && A.running) {
            int i;
            for (i = Y2K_LEDS; i <= Y2K_LEDS; i++) InvalidateRect(GetDlgItem(hw, Y2K_ID(i)), NULL, FALSE);
        }
        if (wp == 3) {
            KillTimer(hw, 3);
            check_updates(0);
        }
        return 0;
    case WM_PF_DONE: {
        Pf *p = (Pf *)lp;
        if (p->gen != A.gen || p != A.pending) {
            pf_free(p);
            return 0;
        }
        A.pending = NULL;
        pf_free(A.pf);
        A.pf = p;
        show_check(p);
        if (A.go_when_ready) go();
        return 0;
    }
    case WM_TICK: on_tick(); return 0;
    case WM_RUN_DONE: on_run_done(); return 0;
    case WM_UPD_DONE: on_update_result(wp, lp); return 0;
    case WM_DL_PROGRESS: {
        char *a = human((u64)wp << 10), *b = human((u64)lp << 10), *t = zs_fmt("Downloading the update: %s of %s", a, lp ? b : "?");
        status_text(t);
        zs_free(a), zs_free(b), zs_free(t);
        return 0;
    }
    case WM_DL_DONE: on_download_done(); return 0;
    case WM_DROPFILES: {
        PtrVec v = {0};
        uw_get_drops((HDROP)wp, &v);
        if (A.page == P_DONE && !A.running) new_archive();
        if (A.page == P_SETUP) add_sources(&v);
        pv_free(&v, 1);
        return 0;
    }
    case WM_SIZE: uw_send(A.sb, WM_SIZE, 0, 0); return 0;
    case WM_ERASEBKGND: {
        RECT rc;
        GetClientRect(hw, &rc);
        FillRect((HDC)wp, &rc, A.face);
        return 1;
    }
    case WM_CLOSE:
        if (A.running) {
            if (A.kind == RUN_JOBS && !ask("A job is running. Pause it and close ZipShrink?\r\n\r\nEverything archived so far is kept safely, and the job can be resumed later.", MB_ICONQUESTION))
                return 0;
            A.closing = 1;
            A.cancel = 1;
            enable(ID_PAUSE, 0);
            status_text("Stopping at the next safe point...");
            return 0;
        }
        DestroyWindow(hw);
        return 0;
    case WM_DESTROY: PostQuitMessage(0); return 0;
    }
    return uw_defproc(hw, m, wp, lp);
}

static int run(void) {
    INITCOMMONCONTROLSEX icc;
    typedef BOOL(WINAPI * ICCEX)(INITCOMMONCONTROLSEX *);
    ICCEX iccex;
    RECT r;
    int argc, i;
    char **argv;
    HICON icon = LoadIconA(GetModuleHandleA(NULL), MAKEINTRESOURCEA(1));
    uw_init();
    /* comctl32 4.70 (Internet Explorer 3) adds InitCommonControlsEx; plain
     * Windows 95 has only InitCommonControls. */
    InitCommonControls();
    iccex = (ICCEX)zs_proc("comctl32.dll", "InitCommonControlsEx");
    if (iccex) {
        icc.dwSize = sizeof(icc), icc.dwICC = ICC_LISTVIEW_CLASSES | ICC_BAR_CLASSES | ICC_PROGRESS_CLASS | ICC_UPDOWN_CLASS;
        iccex(&icc);
    }
    InitializeCriticalSection(&A.cs);
    A.font = CreateFontA(-11, 0, 0, 0, FW_NORMAL, 0, 0, 0, ANSI_CHARSET, OUT_DEFAULT_PRECIS, CLIP_DEFAULT_PRECIS, NONANTIALIASED_QUALITY, DEFAULT_PITCH | FF_SWISS, "MS Sans Serif");
    A.bold = CreateFontA(-11, 0, 0, 0, FW_BOLD, 0, 0, 0, ANSI_CHARSET, OUT_DEFAULT_PRECIS, CLIP_DEFAULT_PRECIS, NONANTIALIASED_QUALITY, DEFAULT_PITCH | FF_SWISS, "MS Sans Serif");
    A.face = GetSysColorBrush(COLOR_BTNFACE);
    y2k_register();
    uw_register(0, wnd_proc, icon, (HBRUSH)(COLOR_BTNFACE + 1), NULL, IDM_MAIN, "ZipShrinkMain");
    SetRect(&r, 0, 0, CW, CH);
    AdjustWindowRectEx(&r, WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX, TRUE, 0);
    A.w = uw_create(0, "ZipShrinkMain", "ZipShrink", WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX, CW_USEDEFAULT, CW_USEDEFAULT,
                    r.right - r.left, r.bottom - r.top, NULL, 0);
    if (!A.w) return 1;
    build();
    refresh_sources();
    show_page(P_SETUP);
    clear_space("Add files or folders to see whether the archive will fit.");
    start_check();
    uw_accept_drops(A.w);
    CheckMenuItem(GetMenu(A.w), IDM_AUTOUPDATE, uw_setting("CheckForUpdates", 1) ? MF_CHECKED : MF_UNCHECKED);
    ShowWindow(A.w, SW_SHOWNORMAL);
    UpdateWindow(A.w);
    /* Paths on the command line (Explorer's "Compress with ZipShrink..."). */
    argv = zs_args(&argc);
    if (argc == 2 && (zs_ends_with_i(argv[1], ".partial") || zs_ends_with_i(argv[1], ".journal"))) {
        resume_path(argv[1]);
    } else if (argc > 1) {
        PtrVec v = {0};
        for (i = 1; i < argc; i++) pv_push(&v, zs_strdup(argv[i]));
        add_sources(&v);
        pv_free(&v, 1);
    }
    /* At most once a day, a few seconds after starting. */
    if (uw_setting("CheckForUpdates", 1) && (u32)zs_unix_now() - uw_setting("LastUpdateCheck", 0) >= 86400) SetTimer(A.w, 3, 3000, NULL);
    return uw_loop(A.w);
}

void zs_gui_start(void) { ExitProcess((UINT)run()); }
