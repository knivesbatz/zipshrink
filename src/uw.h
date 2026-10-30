/* User-interface calls with UTF-8 text, for Windows 95 through 11.
 *
 * On the NT family the windows are Unicode (every W function is looked up at
 * run time, so the import table only names functions Windows 95 has); on
 * Windows 95/98/Me they are ANSI windows in the system code page. */
#ifndef ZS_UW_H
#define ZS_UW_H
#include "base.h"
#include <commctrl.h>
#include <commdlg.h>
#include <shellapi.h>

void uw_init(void);
int uw_unicode(void);

ATOM uw_register(UINT style, WNDPROC proc, HICON icon, HBRUSH bg, const char *menu_name, int menu_id, const char *name);
HWND uw_create(DWORD ex, const char *cls, const char *text, DWORD style, int x, int y, int w, int h, HWND parent, int id);
LRESULT uw_defproc(HWND h, UINT m, WPARAM w, LPARAM l);
LRESULT uw_send(HWND h, UINT m, WPARAM w, LPARAM l);
BOOL uw_post(HWND h, UINT m, WPARAM w, LPARAM l);
int uw_loop(HWND main);
INT_PTR uw_dialog(int id, HWND parent, DLGPROC proc, LPARAM param);
LRESULT uw_call_proc(WNDPROC proc, HWND h, UINT m, WPARAM w, LPARAM l);
WNDPROC uw_subclass(HWND h, WNDPROC proc);

void uw_settext(HWND h, const char *s);
char *uw_gettext(HWND h); /* heap */
void uw_setitemtext(HWND dlg, int id, const char *s);
int uw_msgbox(HWND h, const char *text, const char *title, UINT type);
void uw_draw_text(HDC dc, const char *s, RECT *r, UINT fmt);
void uw_text_out(HDC dc, int x, int y, const char *s);
SIZE uw_text_size(HDC dc, const char *s);

/* List views. */
void lv_column(HWND lv, int col, const char *title, int width, int right);
int lv_insert(HWND lv, int row, const char *text, LPARAM param);
void lv_set(HWND lv, int row, int col, const char *text);
/* Status bar, combo box. */
void sb_text(HWND sb, int part, const char *text);
void cb_add(HWND cb, const char *text);

/* Common dialogs. "Name|*.zip|All files|*.*" filters. Each result is a
 * heap UTF-8 path, pushed onto `out`. 0 if cancelled. */
int uw_open_files(HWND owner, const char *title, const char *filter, int multi, PtrVec *out);
char *uw_pick_folder(HWND owner, const char *title); /* heap or NULL */
void uw_shell_open(const char *path);
void uw_accept_drops(HWND h);
void uw_get_drops(HDROP d, PtrVec *out);

/* Small settings in HKEY_CURRENT_USER\Software\ZipShrink. */
DWORD uw_setting(const char *name, DWORD def);
void uw_set_setting(const char *name, DWORD v);
#endif
