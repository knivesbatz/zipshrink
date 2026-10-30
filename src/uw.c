#include "uw.h"
#include <shlobj.h>

typedef ATOM(WINAPI *RegisterClassW_t)(const WNDCLASSW *);
typedef HWND(WINAPI *CreateWindowExW_t)(DWORD, const WCHAR *, const WCHAR *, DWORD, int, int, int, int, HWND, HMENU, HINSTANCE, void *);
typedef LRESULT(WINAPI *Proc_t)(HWND, UINT, WPARAM, LPARAM);
typedef BOOL(WINAPI *PostMessageW_t)(HWND, UINT, WPARAM, LPARAM);
typedef BOOL(WINAPI *GetMessageW_t)(MSG *, HWND, UINT, UINT);
typedef LRESULT(WINAPI *DispatchMessageW_t)(const MSG *);
typedef BOOL(WINAPI *IsDialogMessageW_t)(HWND, MSG *);
typedef INT_PTR(WINAPI *DialogBoxParamW_t)(HINSTANCE, const WCHAR *, HWND, DLGPROC, LPARAM);
typedef BOOL(WINAPI *SetWindowTextW_t)(HWND, const WCHAR *);
typedef int(WINAPI *GetWindowTextW_t)(HWND, WCHAR *, int);
typedef int(WINAPI *GetWindowTextLengthW_t)(HWND);
typedef BOOL(WINAPI *SetDlgItemTextW_t)(HWND, int, const WCHAR *);
typedef int(WINAPI *MessageBoxW_t)(HWND, const WCHAR *, const WCHAR *, UINT);
typedef int(WINAPI *DrawTextW_t)(HDC, const WCHAR *, int, RECT *, UINT);
typedef BOOL(WINAPI *TextOutW_t)(HDC, int, int, const WCHAR *, int);
typedef BOOL(WINAPI *GetTextExtentPoint32W_t)(HDC, const WCHAR *, int, SIZE *);
typedef LRESULT(WINAPI *CallWindowProcW_t)(WNDPROC, HWND, UINT, WPARAM, LPARAM);
typedef LONG(WINAPI *SetWindowLongW_t)(HWND, int, LONG);
typedef BOOL(WINAPI *GetOpenFileNameW_t)(OPENFILENAMEW *);
typedef BOOL(WINAPI *GetOpenFileNameA_t)(OPENFILENAMEA *);
typedef LPITEMIDLIST(WINAPI *SHBrowseForFolder_t)(void *);
typedef BOOL(WINAPI *SHGetPathFromIDList_t)(LPCITEMIDLIST, void *);
typedef HRESULT(WINAPI *SHGetMalloc_t)(IMalloc **);
typedef HINSTANCE(WINAPI *ShellExecuteW_t)(HWND, const WCHAR *, const WCHAR *, const WCHAR *, const WCHAR *, int);
typedef HINSTANCE(WINAPI *ShellExecuteA_t)(HWND, const char *, const char *, const char *, const char *, int);
typedef UINT(WINAPI *DragQueryFile_t)(HDROP, UINT, void *, UINT);
typedef void(WINAPI *DragAcceptFiles_t)(HWND, BOOL);
typedef void(WINAPI *DragFinish_t)(HDROP);

static struct {
    int unicode;
    RegisterClassW_t RegisterClassW;
    CreateWindowExW_t CreateWindowExW;
    Proc_t DefWindowProcW, SendMessageW;
    PostMessageW_t PostMessageW;
    GetMessageW_t GetMessageW;
    DispatchMessageW_t DispatchMessageW;
    IsDialogMessageW_t IsDialogMessageW;
    DialogBoxParamW_t DialogBoxParamW;
    SetWindowTextW_t SetWindowTextW;
    GetWindowTextW_t GetWindowTextW;
    GetWindowTextLengthW_t GetWindowTextLengthW;
    SetDlgItemTextW_t SetDlgItemTextW;
    MessageBoxW_t MessageBoxW;
    DrawTextW_t DrawTextW;
    TextOutW_t TextOutW;
    GetTextExtentPoint32W_t GetTextExtentPoint32W;
    CallWindowProcW_t CallWindowProcW;
    SetWindowLongW_t SetWindowLongW;
    GetOpenFileNameW_t GetOpenFileNameW;
    GetOpenFileNameA_t GetOpenFileNameA;
    SHBrowseForFolder_t SHBrowseForFolder;
    SHGetPathFromIDList_t SHGetPathFromIDList;
    SHGetMalloc_t SHGetMalloc;
    ShellExecuteW_t ShellExecuteW;
    ShellExecuteA_t ShellExecuteA;
    DragQueryFile_t DragQueryFile;
    DragAcceptFiles_t DragAcceptFiles;
    DragFinish_t DragFinish;
} U;

static void *lib_proc(const char *dll, const char *name) {
    HMODULE m = GetModuleHandleA(dll);
    if (!m) m = LoadLibraryA(dll);
    return m ? (void *)GetProcAddress(m, name) : NULL;
}

void uw_init(void) {
    const char *u = "user32.dll", *g = "gdi32.dll";
    LoadLibraryA("comdlg32.dll");
    LoadLibraryA("shell32.dll");
    if (zs_is_nt()) {
        U.RegisterClassW = (RegisterClassW_t)zs_proc(u, "RegisterClassW");
        U.CreateWindowExW = (CreateWindowExW_t)zs_proc(u, "CreateWindowExW");
        U.DefWindowProcW = (Proc_t)zs_proc(u, "DefWindowProcW");
        U.SendMessageW = (Proc_t)zs_proc(u, "SendMessageW");
        U.PostMessageW = (PostMessageW_t)zs_proc(u, "PostMessageW");
        U.GetMessageW = (GetMessageW_t)zs_proc(u, "GetMessageW");
        U.DispatchMessageW = (DispatchMessageW_t)zs_proc(u, "DispatchMessageW");
        U.IsDialogMessageW = (IsDialogMessageW_t)zs_proc(u, "IsDialogMessageW");
        U.DialogBoxParamW = (DialogBoxParamW_t)zs_proc(u, "DialogBoxParamW");
        U.SetWindowTextW = (SetWindowTextW_t)zs_proc(u, "SetWindowTextW");
        U.GetWindowTextW = (GetWindowTextW_t)zs_proc(u, "GetWindowTextW");
        U.GetWindowTextLengthW = (GetWindowTextLengthW_t)zs_proc(u, "GetWindowTextLengthW");
        U.SetDlgItemTextW = (SetDlgItemTextW_t)zs_proc(u, "SetDlgItemTextW");
        U.MessageBoxW = (MessageBoxW_t)zs_proc(u, "MessageBoxW");
        U.DrawTextW = (DrawTextW_t)zs_proc(u, "DrawTextW");
        U.CallWindowProcW = (CallWindowProcW_t)zs_proc(u, "CallWindowProcW");
        U.SetWindowLongW = (SetWindowLongW_t)zs_proc(u, "SetWindowLongW");
        U.TextOutW = (TextOutW_t)zs_proc(g, "TextOutW");
        U.GetTextExtentPoint32W = (GetTextExtentPoint32W_t)zs_proc(g, "GetTextExtentPoint32W");
        U.GetOpenFileNameW = (GetOpenFileNameW_t)lib_proc("comdlg32.dll", "GetOpenFileNameW");
        U.SHBrowseForFolder = (SHBrowseForFolder_t)lib_proc("shell32.dll", "SHBrowseForFolderW");
        U.SHGetPathFromIDList = (SHGetPathFromIDList_t)lib_proc("shell32.dll", "SHGetPathFromIDListW");
        U.ShellExecuteW = (ShellExecuteW_t)lib_proc("shell32.dll", "ShellExecuteW");
        U.DragQueryFile = (DragQueryFile_t)lib_proc("shell32.dll", "DragQueryFileW");
        U.unicode = U.RegisterClassW && U.CreateWindowExW && U.DefWindowProcW && U.SendMessageW && U.PostMessageW && U.GetMessageW && U.DispatchMessageW &&
                    U.IsDialogMessageW && U.DialogBoxParamW && U.SetWindowTextW && U.GetWindowTextW && U.GetWindowTextLengthW && U.SetDlgItemTextW &&
                    U.MessageBoxW && U.DrawTextW && U.CallWindowProcW && U.SetWindowLongW && U.TextOutW && U.GetTextExtentPoint32W && U.GetOpenFileNameW &&
                    U.SHBrowseForFolder && U.SHGetPathFromIDList && U.ShellExecuteW && U.DragQueryFile;
    }
    if (!U.unicode) {
        /* Windows 95 exports some of these without the A suffix. */
        U.GetOpenFileNameA = (GetOpenFileNameA_t)lib_proc("comdlg32.dll", "GetOpenFileNameA");
        U.SHBrowseForFolder = (SHBrowseForFolder_t)lib_proc("shell32.dll", "SHBrowseForFolderA");
        if (!U.SHBrowseForFolder) U.SHBrowseForFolder = (SHBrowseForFolder_t)lib_proc("shell32.dll", "SHBrowseForFolder");
        U.SHGetPathFromIDList = (SHGetPathFromIDList_t)lib_proc("shell32.dll", "SHGetPathFromIDListA");
        if (!U.SHGetPathFromIDList) U.SHGetPathFromIDList = (SHGetPathFromIDList_t)lib_proc("shell32.dll", "SHGetPathFromIDList");
        U.ShellExecuteA = (ShellExecuteA_t)lib_proc("shell32.dll", "ShellExecuteA");
        U.DragQueryFile = (DragQueryFile_t)lib_proc("shell32.dll", "DragQueryFileA");
        if (!U.DragQueryFile) U.DragQueryFile = (DragQueryFile_t)lib_proc("shell32.dll", "DragQueryFile");
    }
    U.SHGetMalloc = (SHGetMalloc_t)lib_proc("shell32.dll", "SHGetMalloc");
    U.DragAcceptFiles = (DragAcceptFiles_t)lib_proc("shell32.dll", "DragAcceptFiles");
    U.DragFinish = (DragFinish_t)lib_proc("shell32.dll", "DragFinish");
}

int uw_unicode(void) { return U.unicode; }

/* Text in the window's character set; free with zs_free. */
static void *tx(const char *s) {
    int lossy;
    if (!s) return NULL;
    if (U.unicode) return zs_utf8_to_wide(s);
    return zs_utf8_to_acp(s, &lossy);
}

ATOM uw_register(UINT style, WNDPROC proc, HICON icon, HBRUSH bg, const char *menu_name, int menu_id, const char *name) {
    HINSTANCE inst = GetModuleHandleA(NULL);
    if (U.unicode) {
        WNDCLASSW c;
        WCHAR *n = zs_utf8_to_wide(name);
        ATOM a;
        memset(&c, 0, sizeof(c));
        c.style = style, c.lpfnWndProc = proc, c.hInstance = inst, c.hIcon = icon, c.hCursor = LoadCursorA(NULL, (const char *)IDC_ARROW);
        c.hbrBackground = bg, c.lpszClassName = n, c.lpszMenuName = menu_id ? (const WCHAR *)MAKEINTRESOURCEW(menu_id) : NULL;
        a = U.RegisterClassW(&c);
        zs_free(n);
        (void)menu_name;
        return a;
    } else {
        WNDCLASSA c;
        memset(&c, 0, sizeof(c));
        c.style = style, c.lpfnWndProc = proc, c.hInstance = inst, c.hIcon = icon, c.hCursor = LoadCursorA(NULL, (const char *)IDC_ARROW);
        c.hbrBackground = bg, c.lpszClassName = name, c.lpszMenuName = menu_id ? MAKEINTRESOURCEA(menu_id) : NULL;
        return RegisterClassA(&c);
    }
}

HWND uw_create(DWORD ex, const char *cls, const char *text, DWORD style, int x, int y, int w, int h, HWND parent, int id) {
    HINSTANCE inst = GetModuleHandleA(NULL);
    HMENU menu = (style & WS_CHILD) ? (HMENU)(INT_PTR)id : NULL;
    HWND r;
    if (U.unicode) {
        WCHAR *c = zs_utf8_to_wide(cls), *t = zs_utf8_to_wide(text ? text : "");
        r = U.CreateWindowExW(ex, c, t, style, x, y, w, h, parent, menu, inst, NULL);
        zs_free(c), zs_free(t);
    } else {
        char *t = tx(text ? text : "");
        r = CreateWindowExA(ex, cls, t, style, x, y, w, h, parent, menu, inst, NULL);
        zs_free(t);
    }
    return r;
}

LRESULT uw_defproc(HWND h, UINT m, WPARAM w, LPARAM l) { return U.unicode ? U.DefWindowProcW(h, m, w, l) : DefWindowProcA(h, m, w, l); }
LRESULT uw_send(HWND h, UINT m, WPARAM w, LPARAM l) { return U.unicode ? U.SendMessageW(h, m, w, l) : SendMessageA(h, m, w, l); }
BOOL uw_post(HWND h, UINT m, WPARAM w, LPARAM l) { return U.unicode ? U.PostMessageW(h, m, w, l) : PostMessageA(h, m, w, l); }
LRESULT uw_call_proc(WNDPROC p, HWND h, UINT m, WPARAM w, LPARAM l) { return U.unicode ? U.CallWindowProcW(p, h, m, w, l) : CallWindowProcA(p, h, m, w, l); }

WNDPROC uw_subclass(HWND h, WNDPROC proc) {
    return (WNDPROC)(U.unicode ? U.SetWindowLongW(h, GWL_WNDPROC, (LONG)(INT_PTR)proc) : SetWindowLongA(h, GWL_WNDPROC, (LONG)(INT_PTR)proc));
}

int uw_loop(HWND main) {
    MSG m;
    for (;;) {
        BOOL r = U.unicode ? U.GetMessageW(&m, NULL, 0, 0) : GetMessageA(&m, NULL, 0, 0);
        HWND top;
        if (r <= 0) break;
        /* Tab and Enter navigation in the main window and in dialogs. */
        top = m.hwnd;
        while (top && (GetWindowLongA(top, GWL_STYLE) & WS_CHILD)) top = GetParent(top);
        if (top && (U.unicode ? U.IsDialogMessageW(top, &m) : IsDialogMessageA(top, &m))) continue;
        TranslateMessage(&m);
        if (U.unicode) U.DispatchMessageW(&m);
        else DispatchMessageA(&m);
    }
    (void)main;
    return (int)m.wParam;
}

INT_PTR uw_dialog(int id, HWND parent, DLGPROC proc, LPARAM param) {
    HINSTANCE inst = GetModuleHandleA(NULL);
    return U.unicode ? U.DialogBoxParamW(inst, (const WCHAR *)MAKEINTRESOURCEW(id), parent, proc, param) : DialogBoxParamA(inst, MAKEINTRESOURCEA(id), parent, proc, param);
}

void uw_settext(HWND h, const char *s) {
    void *t = tx(s ? s : "");
    if (U.unicode) U.SetWindowTextW(h, t);
    else SetWindowTextA(h, t);
    zs_free(t);
}

char *uw_gettext(HWND h) {
    if (U.unicode) {
        int n = U.GetWindowTextLengthW(h);
        WCHAR *w = zs_alloc(((size_t)n + 2) * sizeof(WCHAR));
        char *r;
        w[0] = 0;
        U.GetWindowTextW(h, w, n + 1);
        r = zs_wide_to_utf8(w);
        zs_free(w);
        return r;
    } else {
        int n = GetWindowTextLengthA(h);
        char *a = zs_alloc((size_t)n + 2), *r;
        a[0] = 0;
        GetWindowTextA(h, a, n + 1);
        r = zs_acp_to_utf8(a);
        zs_free(a);
        return r;
    }
}

void uw_setitemtext(HWND dlg, int id, const char *s) {
    void *t = tx(s);
    if (U.unicode) U.SetDlgItemTextW(dlg, id, t);
    else SetDlgItemTextA(dlg, id, t);
    zs_free(t);
}

int uw_msgbox(HWND h, const char *text, const char *title, UINT type) {
    void *a = tx(text), *b = tx(title);
    int r = U.unicode ? U.MessageBoxW(h, a, b, type) : MessageBoxA(h, a, b, type);
    zs_free(a), zs_free(b);
    return r;
}

void uw_draw_text(HDC dc, const char *s, RECT *r, UINT fmt) {
    void *t = tx(s);
    if (U.unicode) U.DrawTextW(dc, t, -1, r, fmt);
    else DrawTextA(dc, t, -1, r, fmt);
    zs_free(t);
}

void uw_text_out(HDC dc, int x, int y, const char *s) {
    void *t = tx(s);
    if (U.unicode) U.TextOutW(dc, x, y, t, (int)zs_wcslen(t));
    else TextOutA(dc, x, y, t, (int)strlen(t));
    zs_free(t);
}

SIZE uw_text_size(HDC dc, const char *s) {
    SIZE z = {0, 0};
    void *t = tx(s);
    if (U.unicode) U.GetTextExtentPoint32W(dc, t, (int)zs_wcslen(t), &z);
    else GetTextExtentPoint32A(dc, t, (int)strlen(t), &z);
    zs_free(t);
    return z;
}

void lv_column(HWND lv, int col, const char *title, int width, int right) {
    void *t = tx(title);
    if (U.unicode) {
        LVCOLUMNW c;
        memset(&c, 0, sizeof(c));
        c.mask = LVCF_TEXT | LVCF_WIDTH | LVCF_FMT, c.fmt = right ? LVCFMT_RIGHT : LVCFMT_LEFT, c.cx = width, c.pszText = t;
        U.SendMessageW(lv, LVM_INSERTCOLUMNW, (WPARAM)col, (LPARAM)&c);
    } else {
        LVCOLUMNA c;
        memset(&c, 0, sizeof(c));
        c.mask = LVCF_TEXT | LVCF_WIDTH | LVCF_FMT, c.fmt = right ? LVCFMT_RIGHT : LVCFMT_LEFT, c.cx = width, c.pszText = t;
        SendMessageA(lv, LVM_INSERTCOLUMNA, (WPARAM)col, (LPARAM)&c);
    }
    zs_free(t);
}

int lv_insert(HWND lv, int row, const char *text, LPARAM param) {
    void *t = tx(text);
    int r;
    if (U.unicode) {
        LVITEMW it;
        memset(&it, 0, sizeof(it));
        it.mask = LVIF_TEXT | LVIF_PARAM, it.iItem = row, it.pszText = t, it.lParam = param;
        r = (int)U.SendMessageW(lv, LVM_INSERTITEMW, 0, (LPARAM)&it);
    } else {
        LVITEMA it;
        memset(&it, 0, sizeof(it));
        it.mask = LVIF_TEXT | LVIF_PARAM, it.iItem = row, it.pszText = t, it.lParam = param;
        r = (int)SendMessageA(lv, LVM_INSERTITEMA, 0, (LPARAM)&it);
    }
    zs_free(t);
    return r;
}

void lv_set(HWND lv, int row, int col, const char *text) {
    void *t = tx(text);
    if (U.unicode) {
        LVITEMW it;
        memset(&it, 0, sizeof(it));
        it.iSubItem = col, it.pszText = t;
        U.SendMessageW(lv, LVM_SETITEMTEXTW, (WPARAM)row, (LPARAM)&it);
    } else {
        LVITEMA it;
        memset(&it, 0, sizeof(it));
        it.iSubItem = col, it.pszText = t;
        SendMessageA(lv, LVM_SETITEMTEXTA, (WPARAM)row, (LPARAM)&it);
    }
    zs_free(t);
}

void sb_text(HWND sb, int part, const char *text) {
    void *t = tx(text);
    if (U.unicode) U.SendMessageW(sb, SB_SETTEXTW, (WPARAM)part, (LPARAM)t);
    else SendMessageA(sb, SB_SETTEXTA, (WPARAM)part, (LPARAM)t);
    zs_free(t);
}

void cb_add(HWND cb, const char *text) {
    void *t = tx(text);
    uw_send(cb, CB_ADDSTRING, 0, (LPARAM)t);
    zs_free(t);
}

/* "A|*.a|B|*.b" -> "A\0*.a\0B\0*.b\0\0" in the window character set. */
static void *filter_tx(const char *f) {
    Buf b = {0};
    void *t, *r;
    size_t n, i, unit = U.unicode ? 2 : 1;
    buf_puts(&b, f);
    buf_puts(&b, "|");
    t = tx(buf_cstr(&b));
    n = U.unicode ? zs_wcslen(t) : strlen(t);
    r = zs_zalloc((n + 2) * unit);
    memcpy(r, t, n * unit);
    for (i = 0; i < n; i++) {
        if (U.unicode && ((WCHAR *)r)[i] == '|') ((WCHAR *)r)[i] = 0;
        if (!U.unicode && ((char *)r)[i] == '|') ((char *)r)[i] = 0;
    }
    zs_free(t);
    buf_free(&b);
    return r;
}

int uw_open_files(HWND owner, const char *title, const char *filter, int multi, PtrVec *out) {
    const size_t cap = 65536;
    void *f = filter_tx(filter), *ti = tx(title);
    int ok;
    DWORD flags = OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_HIDEREADONLY | OFN_EXPLORER | OFN_NOCHANGEDIR | (multi ? OFN_ALLOWMULTISELECT : 0);
    if (U.unicode) {
        OPENFILENAMEW o;
        WCHAR *buf = zs_zalloc(cap * sizeof(WCHAR));
        memset(&o, 0, sizeof(o));
        o.lStructSize = sizeof(o), o.hwndOwner = owner, o.lpstrFilter = f, o.lpstrFile = buf, o.nMaxFile = (DWORD)cap, o.lpstrTitle = ti, o.Flags = flags;
        ok = U.GetOpenFileNameW(&o);
        if (ok) {
            const WCHAR *p = buf + zs_wcslen(buf) + 1;
            char *dir = zs_wide_to_utf8(buf);
            if (!*p) pv_push(out, dir);
            else {
                for (; *p; p += zs_wcslen(p) + 1) {
                    char *n = zs_wide_to_utf8(p);
                    pv_push(out, zs_path_join(dir, n));
                    zs_free(n);
                }
                zs_free(dir);
            }
        }
        zs_free(buf);
    } else {
        OPENFILENAMEA o;
        char *buf = zs_zalloc(cap);
        memset(&o, 0, sizeof(o));
        /* The Windows 95 structure size: without the Windows 2000 fields. */
        o.lStructSize = OPENFILENAME_SIZE_VERSION_400A, o.hwndOwner = owner, o.lpstrFilter = f, o.lpstrFile = buf, o.nMaxFile = (DWORD)cap, o.lpstrTitle = ti, o.Flags = flags;
        ok = U.GetOpenFileNameA && U.GetOpenFileNameA(&o);
        if (ok) {
            const char *p = buf + strlen(buf) + 1;
            char *dir = zs_acp_to_utf8(buf);
            if (!*p) pv_push(out, dir);
            else {
                for (; *p; p += strlen(p) + 1) {
                    char *n = zs_acp_to_utf8(p);
                    pv_push(out, zs_path_join(dir, n));
                    zs_free(n);
                }
                zs_free(dir);
            }
        }
        zs_free(buf);
    }
    zs_free(f), zs_free(ti);
    return ok;
}

char *uw_pick_folder(HWND owner, const char *title) {
    char *r = NULL;
    void *ti = tx(title);
    LPITEMIDLIST id;
    if (!U.SHBrowseForFolder || !U.SHGetPathFromIDList) return zs_free(ti), NULL;
    if (U.unicode) {
        BROWSEINFOW b;
        WCHAR name[MAX_PATH];
        memset(&b, 0, sizeof(b));
        b.hwndOwner = owner, b.pszDisplayName = name, b.lpszTitle = ti, b.ulFlags = BIF_RETURNONLYFSDIRS;
        id = U.SHBrowseForFolder(&b);
        if (id) {
            WCHAR path[MAX_PATH + 1];
            if (U.SHGetPathFromIDList(id, path)) r = zs_wide_to_utf8(path);
        }
    } else {
        BROWSEINFOA b;
        char name[MAX_PATH];
        memset(&b, 0, sizeof(b));
        b.hwndOwner = owner, b.pszDisplayName = name, b.lpszTitle = ti, b.ulFlags = BIF_RETURNONLYFSDIRS;
        id = U.SHBrowseForFolder(&b);
        if (id) {
            char path[MAX_PATH + 1];
            if (U.SHGetPathFromIDList(id, path)) r = zs_acp_to_utf8(path);
        }
    }
    if (id && U.SHGetMalloc) {
        IMalloc *m;
        if (U.SHGetMalloc(&m) == S_OK) m->lpVtbl->Free(m, id), m->lpVtbl->Release(m);
    }
    zs_free(ti);
    return r;
}

void uw_shell_open(const char *path) {
    void *t = tx(path), *verb = tx("open");
    if (U.unicode) U.ShellExecuteW(NULL, verb, t, NULL, NULL, SW_SHOWNORMAL);
    else if (U.ShellExecuteA) U.ShellExecuteA(NULL, verb, t, NULL, NULL, SW_SHOWNORMAL);
    zs_free(t), zs_free(verb);
}

void uw_accept_drops(HWND h) {
    if (U.DragAcceptFiles) U.DragAcceptFiles(h, TRUE);
}

void uw_get_drops(HDROP d, PtrVec *out) {
    UINT i, n;
    if (!U.DragQueryFile) return;
    n = U.DragQueryFile(d, 0xFFFFFFFFu, NULL, 0);
    for (i = 0; i < n; i++) {
        if (U.unicode) {
            WCHAR buf[MAX_PATH * 4];
            if (U.DragQueryFile(d, i, buf, ARRAY_LEN(buf))) pv_push(out, zs_wide_to_utf8(buf));
        } else {
            char buf[MAX_PATH + 1];
            if (U.DragQueryFile(d, i, buf, sizeof(buf))) pv_push(out, zs_acp_to_utf8(buf));
        }
    }
    if (U.DragFinish) U.DragFinish(d);
}

DWORD uw_setting(const char *name, DWORD def) {
    HKEY k;
    DWORD v = def, type, n = sizeof(v);
    if (RegOpenKeyExA(HKEY_CURRENT_USER, "Software\\ZipShrink", 0, KEY_READ, &k) == ERROR_SUCCESS) {
        if (RegQueryValueExA(k, name, NULL, &type, (BYTE *)&v, &n) != ERROR_SUCCESS || type != REG_DWORD) v = def;
        RegCloseKey(k);
    }
    return v;
}

void uw_set_setting(const char *name, DWORD v) {
    HKEY k;
    DWORD disp;
    if (RegCreateKeyExA(HKEY_CURRENT_USER, "Software\\ZipShrink", 0, NULL, 0, KEY_WRITE, NULL, &k, &disp) == ERROR_SUCCESS) {
        RegSetValueExA(k, name, 0, REG_DWORD, (const BYTE *)&v, sizeof(v));
        RegCloseKey(k);
    }
}
