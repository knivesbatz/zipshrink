#!/usr/bin/env python3
"""Fails if an executable imports a DLL or function that is not on the list
below: functions present in the first release of Windows 95 (4.00.950).

A program that imports a function Windows 95 lacks does not start there at
all ("The ... file is linked to missing export"), so every import is checked
by name. Newer functions must be looked up at run time with GetProcAddress
(zs_proc) and used only when present.

    python3 scripts/check_imports.py build/zipshrink.exe build/zipshrink-gui.exe
"""
import struct
import sys

WIN95 = {
    "KERNEL32.DLL": """
        CloseHandle CreateDirectoryA CreateFileA CreateProcessA CreateThread DeleteCriticalSection DeleteFileA
        EnterCriticalSection ExitProcess FileTimeToDosDateTime FileTimeToLocalFileTime FindClose FindFirstFileA
        FindNextFileA FlushFileBuffers FormatMessageA GetCommandLineA GetConsoleMode GetCurrentProcess
        GetCurrentProcessId GetDiskFreeSpaceA GetEnvironmentVariableA GetFileAttributesA GetFileInformationByHandle
        GetFileSize GetFileTime GetFileType GetFullPathNameA GetLastError GetModuleFileNameA GetModuleHandleA
        GetProcAddress GetProcessHeap GetStdHandle GetSystemTimeAsFileTime GetTempPathA GetTickCount GetVersion
        GetVersionExA GetVolumeInformationA HeapAlloc HeapFree HeapReAlloc InitializeCriticalSection
        LeaveCriticalSection LoadLibraryA LockFile MoveFileA MultiByteToWideChar ReadFile RemoveDirectoryA
        SetConsoleCtrlHandler SetEndOfFile SetFileAttributesA SetFilePointer SetFileTime SetLastError Sleep
        TerminateProcess UnlockFile VirtualAlloc VirtualFree WaitForSingleObject WideCharToMultiByte WriteFile
        lstrcmpA lstrcmpiA lstrlenA
    """,
    "USER32.DLL": """
        AdjustWindowRectEx BeginPaint CallWindowProcA CheckMenuItem CreateWindowExA DefWindowProcA DestroyWindow
        DialogBoxParamA DispatchMessageA DrawEdge DrawTextA EnableMenuItem EnableWindow EndDialog EndPaint
        FillRect GetClientRect GetDlgCtrlID GetDlgItem GetMenu GetMessageA GetParent GetSysColor GetSysColorBrush
        GetWindowLongA GetWindowTextA GetWindowTextLengthA InvalidateRect IsDialogMessageA IsDlgButtonChecked
        KillTimer LoadCursorA LoadIconA MessageBeep MessageBoxA PostMessageA PostQuitMessage RegisterClassA
        SendMessageA SetCursor SetDlgItemTextA SetRect SetTimer SetWindowLongA SetWindowTextA ShowWindow
        TranslateMessage UpdateWindow
    """,
    "GDI32.DLL": """
        Arc BitBlt CreateCompatibleBitmap CreateCompatibleDC CreateFontA CreateFontIndirectA CreatePen
        CreateSolidBrush DeleteDC DeleteObject Ellipse GetObjectA GetStockObject GetTextExtentPoint32A
        IntersectClipRect LineTo MoveToEx Polygon RestoreDC SaveDC SelectObject SetBkColor SetBkMode SetPixel
        SetTextColor TextOutA
    """,
    "COMCTL32.DLL": "InitCommonControls",
    "ADVAPI32.DLL": "RegCloseKey RegCreateKeyExA RegOpenKeyExA RegQueryValueExA RegSetValueExA",
}
ALLOWED = {dll: set(names.split()) for dll, names in WIN95.items()}


def imports(path):
    data = open(path, "rb").read()
    pe = struct.unpack_from("<I", data, 0x3C)[0]
    if data[pe:pe + 4] != b"PE\0\0":
        raise SystemExit(f"{path}: not a PE file")
    nsec = struct.unpack_from("<H", data, pe + 6)[0]
    optsize = struct.unpack_from("<H", data, pe + 20)[0]
    opt = pe + 24
    if struct.unpack_from("<H", data, opt)[0] != 0x10B:
        raise SystemExit(f"{path}: not a 32-bit program")
    major_os, minor_os, _, _, major_sub, minor_sub = struct.unpack_from("<6H", data, opt + 40)
    imp_rva = struct.unpack_from("<I", data, opt + 96 + 8)[0]
    sections = []
    for i in range(nsec):
        s = pe + 24 + optsize + 40 * i
        vsize, va, rawsize, raw = struct.unpack_from("<IIII", data, s + 8)
        sections.append((va, max(vsize, rawsize), raw))

    def off(rva):
        for va, size, raw in sections:
            if va <= rva < va + size:
                return rva - va + raw
        raise SystemExit(f"{path}: bad RVA {rva:#x}")

    def cstr(o):
        return data[o:data.index(b"\0", o)].decode()

    result = {}
    d = off(imp_rva)
    while True:
        ilt, _, _, name, iat = struct.unpack_from("<IIIII", data, d)
        if not name:
            break
        dll = cstr(off(name)).upper()
        names = result.setdefault(dll, [])
        t = off(ilt or iat)
        while True:
            v = struct.unpack_from("<I", data, t)[0]
            if not v:
                break
            names.append(f"#{v & 0xFFFF}" if v & 0x80000000 else cstr(off(v) + 2))
            t += 4
        d += 20
    return result, (major_os, minor_os), (major_sub, minor_sub)


def main(paths):
    bad = 0
    for p in paths:
        imp, osv, subv = imports(p)
        if subv > (4, 0) or osv > (4, 0):
            print(f"{p}: needs Windows {subv[0]}.{subv[1]} (subsystem) / {osv[0]}.{osv[1]} (OS); Windows 95 is 4.0")
            bad += 1
        for dll, names in sorted(imp.items()):
            if dll not in ALLOWED:
                print(f"{p}: imports {dll}, which is not on the Windows 95 list")
                bad += 1
                continue
            for n in names:
                if n not in ALLOWED[dll]:
                    print(f"{p}: imports {dll}!{n}, which is not on the Windows 95 list")
                    bad += 1
        total = sum(len(v) for v in imp.values())
        print(f"{p}: {total} imports from {', '.join(sorted(imp))}; subsystem {subv[0]}.{subv[1]}")
    if bad:
        print(f"{bad} problems")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
