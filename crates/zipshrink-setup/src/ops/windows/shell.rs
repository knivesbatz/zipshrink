//! Known folders and `.lnk` shortcuts.
//!
//! Shortcuts go through the shell's own `IShellLinkW` + `IPersistFile`
//! (what NSIS's CreateShortcut does) rather than a hand-written .lnk
//! encoder: the shell fills in the ID list, the link-tracking data and the
//! icon exactly as Explorer expects on every Windows version. windows-sys
//! has no COM interfaces, so the two vtables are declared here, in the
//! order of ShObjIdl_core.h / ObjIdl.h.

use super::{from_wide, wide};
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use windows_sys::core::{GUID, HRESULT};
use windows_sys::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
    COINIT_DISABLE_OLE1DDE, STGM_READ,
};
use windows_sys::Win32::UI::Shell::{SHGetKnownFolderPath, ShellLink, KF_FLAG_DEFAULT, SLGP_RAWPATH};

/// A known folder (FOLDERID_*), such as the Start menu's Programs.
pub fn known_folder(id: &GUID) -> Option<PathBuf> {
    let mut p: *mut u16 = std::ptr::null_mut();
    // SAFETY: valid GUID and out pointer; the result is freed below.
    let hr = unsafe { SHGetKnownFolderPath(id, KF_FLAG_DEFAULT as u32, std::ptr::null_mut(), &mut p) };
    let out = if hr >= 0 && !p.is_null() {
        // SAFETY: on success p is a NUL-terminated string.
        let s = unsafe { from_wide(p) };
        (!s.is_empty()).then(|| PathBuf::from(s))
    } else {
        None
    };
    // SAFETY: CoTaskMemFree accepts null and what SHGetKnownFolderPath returned.
    unsafe { CoTaskMemFree(p as *const c_void) };
    out
}

const IID_ISHELLLINKW: GUID = GUID::from_u128(0x000214f9_0000_0000_c000_000000000046);
const IID_IPERSISTFILE: GUID = GUID::from_u128(0x0000010b_0000_0000_c000_000000000046);

type Unused = usize;

#[repr(C)]
struct IUnknownVtbl {
    query_interface: unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> HRESULT,
    add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
}

#[repr(C)]
struct IShellLinkWVtbl {
    base: IUnknownVtbl,
    get_path: unsafe extern "system" fn(*mut c_void, *mut u16, i32, *mut c_void, u32) -> HRESULT,
    _get_id_list: Unused,
    _set_id_list: Unused,
    _get_description: Unused,
    set_description: unsafe extern "system" fn(*mut c_void, *const u16) -> HRESULT,
    _get_working_directory: Unused,
    set_working_directory: unsafe extern "system" fn(*mut c_void, *const u16) -> HRESULT,
    _get_arguments: Unused,
    _set_arguments: Unused,
    _get_hotkey: Unused,
    _set_hotkey: Unused,
    _get_show_cmd: Unused,
    _set_show_cmd: Unused,
    _get_icon_location: Unused,
    set_icon_location: unsafe extern "system" fn(*mut c_void, *const u16, i32) -> HRESULT,
    _set_relative_path: Unused,
    _resolve: Unused,
    set_path: unsafe extern "system" fn(*mut c_void, *const u16) -> HRESULT,
}

#[repr(C)]
struct IPersistFileVtbl {
    base: IUnknownVtbl,
    _get_class_id: Unused,
    _is_dirty: Unused,
    load: unsafe extern "system" fn(*mut c_void, *const u16, u32) -> HRESULT,
    save: unsafe extern "system" fn(*mut c_void, *const u16, i32) -> HRESULT,
    _save_completed: Unused,
    _get_cur_file: Unused,
}

/// An owned COM interface pointer whose vtable is `V`; released on drop.
struct Com<V> {
    ptr: *mut c_void,
    _v: std::marker::PhantomData<V>,
}

impl<V> Com<V> {
    /// SAFETY: `ptr` must be a live interface pointer whose vtable starts with `V`'s layout.
    unsafe fn from_raw(ptr: *mut c_void) -> Self {
        Com { ptr, _v: std::marker::PhantomData }
    }
    fn vtbl(&self) -> &V {
        // SAFETY: a COM object starts with a pointer to its vtable.
        unsafe { &**(self.ptr as *const *const V) }
    }
    fn unknown(&self) -> &IUnknownVtbl {
        // SAFETY: every COM vtable starts with IUnknown's three methods.
        unsafe { &**(self.ptr as *const *const IUnknownVtbl) }
    }
    fn query<W>(&self, iid: &GUID) -> Result<Com<W>, HRESULT> {
        let mut out: *mut c_void = std::ptr::null_mut();
        // SAFETY: valid interface pointer, IID and out pointer.
        let hr = unsafe { (self.unknown().query_interface)(self.ptr, iid, &mut out) };
        if hr < 0 || out.is_null() {
            return Err(hr);
        }
        // SAFETY: QueryInterface returned the interface `iid` names, which the caller pairs with W.
        Ok(unsafe { Com::from_raw(out) })
    }
}

impl<V> Drop for Com<V> {
    fn drop(&mut self) {
        // SAFETY: we hold one reference, given back once.
        unsafe { (self.unknown().release)(self.ptr) };
    }
}

/// COM for this thread for as long as it lives.
struct ComScope(bool);

impl ComScope {
    fn new() -> Self {
        // SAFETY: plain call; balanced in drop when it succeeded (S_OK or S_FALSE).
        let hr = unsafe { CoInitializeEx(std::ptr::null(), (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as u32) };
        ComScope(hr >= 0)
    }
}

impl Drop for ComScope {
    fn drop(&mut self) {
        if self.0 {
            // SAFETY: balances the successful CoInitializeEx above.
            unsafe { CoUninitialize() };
        }
    }
}

fn check(hr: HRESULT, what: &str) -> Result<(), String> {
    if hr < 0 {
        return Err(format!("{what} (error 0x{:08X})", hr as u32));
    }
    Ok(())
}

fn shell_link() -> Result<Com<IShellLinkWVtbl>, String> {
    let mut p: *mut c_void = std::ptr::null_mut();
    // SAFETY: valid CLSID/IID and out pointer.
    let hr = unsafe { CoCreateInstance(&ShellLink, std::ptr::null_mut(), CLSCTX_INPROC_SERVER, &IID_ISHELLLINKW, &mut p) };
    check(hr, "could not create a shortcut object")?;
    if p.is_null() {
        return Err("could not create a shortcut object".into());
    }
    // SAFETY: CoCreateInstance returned an IShellLinkW.
    Ok(unsafe { Com::from_raw(p) })
}

/// Writes the shortcut `lnk` to `target` (started in its own folder).
pub fn create_link(lnk: &Path, target: &Path, description: &str) -> Result<(), String> {
    let _com = ComScope::new();
    let fail = |e: String| format!("could not create the shortcut {}: {e}", lnk.display());
    let link = shell_link().map_err(fail)?;
    let t = wide(&target.to_string_lossy());
    let wd = wide(&target.parent().unwrap_or(target).to_string_lossy());
    let d = wide(description);
    let v = link.vtbl();
    // SAFETY: live IShellLinkW; every string is NUL-terminated and outlives the call.
    unsafe {
        check((v.set_path)(link.ptr, t.as_ptr()), "SetPath").map_err(fail)?;
        check((v.set_working_directory)(link.ptr, wd.as_ptr()), "SetWorkingDirectory").map_err(fail)?;
        check((v.set_description)(link.ptr, d.as_ptr()), "SetDescription").map_err(fail)?;
        check((v.set_icon_location)(link.ptr, t.as_ptr(), 0), "SetIconLocation").map_err(fail)?;
    }
    let file: Com<IPersistFileVtbl> = link.query(&IID_IPERSISTFILE).map_err(|hr| fail(format!("no IPersistFile (0x{:08X})", hr as u32)))?;
    if let Some(dir) = lnk.parent() {
        std::fs::create_dir_all(dir).map_err(|e| fail(e.to_string()))?;
    }
    let p = wide(&lnk.to_string_lossy());
    // SAFETY: live IPersistFile; NUL-terminated path.
    check(unsafe { (file.vtbl().save)(file.ptr, p.as_ptr(), 1) }, "Save").map_err(fail)
}

/// The program a shortcut starts, as stored (not resolved), if it can be read.
pub fn link_target(lnk: &Path) -> Option<String> {
    let _com = ComScope::new();
    let link = shell_link().ok()?;
    let file: Com<IPersistFileVtbl> = link.query(&IID_IPERSISTFILE).ok()?;
    let p = wide(&lnk.to_string_lossy());
    // SAFETY: live IPersistFile; NUL-terminated path.
    if unsafe { (file.vtbl().load)(file.ptr, p.as_ptr(), STGM_READ) } < 0 {
        return None;
    }
    let mut buf = vec![0u16; 1024];
    // SAFETY: buf holds buf.len() characters; no WIN32_FIND_DATAW wanted.
    let hr = unsafe { (link.vtbl().get_path)(link.ptr, buf.as_mut_ptr(), buf.len() as i32, std::ptr::null_mut(), SLGP_RAWPATH as u32) };
    if hr != 0 {
        // S_FALSE: the shortcut has no file-system target.
        return None;
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(String::from_utf16_lossy(&buf[..len]))
}
