//! A small, safe wrapper over the parts of the registry API setup uses.
//! Everything lives under HKEY_CURRENT_USER: ZipShrink installs per user.

use super::wide;
use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, ERROR_SUCCESS};
use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteKeyW, RegDeleteTreeW, RegDeleteValueW, RegOpenKeyExW, RegQueryInfoKeyW, RegQueryValueExW,
    RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_DWORD, REG_EXPAND_SZ, REG_OPTION_NON_VOLATILE, REG_SZ,
};

/// An open key under HKEY_CURRENT_USER, closed on drop.
pub struct Key(HKEY);

impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: self.0 came from RegOpenKeyExW/RegCreateKeyExW and is closed once.
        unsafe { RegCloseKey(self.0) };
    }
}

fn err(op: &str, path: &str, code: u32) -> String {
    format!("could not {op} HKEY_CURRENT_USER\\{path} ({})", std::io::Error::from_raw_os_error(code as i32))
}

/// Opens (creating as needed) `path` for writing.
pub fn create(path: &str) -> Result<Key, String> {
    let mut h: HKEY = std::ptr::null_mut();
    let p = wide(path);
    // SAFETY: valid NUL-terminated path and out pointer.
    let r = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            p.as_ptr(),
            0,
            std::ptr::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_READ | KEY_WRITE,
            std::ptr::null(),
            &mut h,
            std::ptr::null_mut(),
        )
    };
    if r != ERROR_SUCCESS {
        return Err(err("create", path, r));
    }
    Ok(Key(h))
}

/// Opens `path`, if it exists, for reading (and writing when `write`).
pub fn open(path: &str, write: bool) -> Option<Key> {
    let mut h: HKEY = std::ptr::null_mut();
    let p = wide(path);
    let access = if write { KEY_READ | KEY_WRITE } else { KEY_READ };
    // SAFETY: valid NUL-terminated path and out pointer.
    let r = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, p.as_ptr(), 0, access, &mut h) };
    (r == ERROR_SUCCESS).then_some(Key(h))
}

/// Reads a string value of `path` ("" names the default value).
pub fn get_str(path: &str, name: &str) -> Option<String> {
    open(path, false)?.get_str(name).map(|(s, _)| s)
}

/// Deletes `path` and everything below it; fine if it is not there.
pub fn delete_tree(path: &str) -> Result<(), String> {
    let p = wide(path);
    // SAFETY: valid NUL-terminated path.
    let r = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, p.as_ptr()) };
    if r != ERROR_SUCCESS && r != ERROR_FILE_NOT_FOUND {
        return Err(err("delete", path, r));
    }
    // RegDeleteTreeW empties the key; remove the key itself too.
    // SAFETY: as above.
    let r = unsafe { RegDeleteKeyW(HKEY_CURRENT_USER, p.as_ptr()) };
    if r != ERROR_SUCCESS && r != ERROR_FILE_NOT_FOUND {
        return Err(err("delete", path, r));
    }
    Ok(())
}

/// Deletes `path` if it holds no values and no subkeys.
pub fn delete_if_empty(path: &str) {
    let empty = open(path, false).is_some_and(|k| k.is_empty());
    if empty {
        let p = wide(path);
        // SAFETY: valid NUL-terminated path.
        unsafe { RegDeleteKeyW(HKEY_CURRENT_USER, p.as_ptr()) };
    }
}

impl Key {
    /// A string value (REG_SZ or REG_EXPAND_SZ, not expanded) and its type.
    pub fn get_str(&self, name: &str) -> Option<(String, u32)> {
        self.read_str(name).ok().flatten()
    }

    /// Like `get_str`, but tells a missing value (`Ok(None)`) from one that
    /// is there and cannot be used as text: another type, not valid
    /// UTF-16, or unreadable (`Err` with the reason).
    pub fn read_str(&self, name: &str) -> Result<Option<(String, u32)>, String> {
        let n = wide(name);
        let mut buf: Vec<u16> = vec![0; 512];
        loop {
            let mut ty = 0u32;
            let mut bytes = (buf.len() * 2) as u32;
            // SAFETY: buf holds `bytes` bytes; the other pointers are valid.
            let r = unsafe { RegQueryValueExW(self.0, n.as_ptr(), std::ptr::null(), &mut ty, buf.as_mut_ptr().cast(), &mut bytes) };
            if r == ERROR_MORE_DATA {
                buf.resize((bytes as usize).div_ceil(2) + 1, 0);
                continue;
            }
            if r == ERROR_FILE_NOT_FOUND {
                return Ok(None);
            }
            if r != ERROR_SUCCESS {
                return Err(format!("could not read the value {name} ({})", std::io::Error::from_raw_os_error(r as i32)));
            }
            if ty != REG_SZ && ty != REG_EXPAND_SZ {
                return Err(format!("the value {name} is not text (registry type {ty})"));
            }
            let mut s = &buf[..(bytes as usize / 2).min(buf.len())];
            while let [rest @ .., 0] = s {
                s = rest;
            }
            let text = String::from_utf16(s).map_err(|_| format!("the value {name} is not valid text"))?;
            return Ok(Some((text, ty)));
        }
    }

    fn set(&self, name: &str, ty: u32, data: &[u8]) -> Result<(), String> {
        let n = wide(name);
        // SAFETY: data is a valid buffer of the given length.
        let r = unsafe { RegSetValueExW(self.0, n.as_ptr(), 0, ty, data.as_ptr(), data.len() as u32) };
        if r != ERROR_SUCCESS {
            return Err(format!("could not write the registry value {name} ({})", std::io::Error::from_raw_os_error(r as i32)));
        }
        Ok(())
    }

    /// Writes a REG_SZ (or, with `ty`, REG_EXPAND_SZ) value.
    pub fn set_str_typed(&self, name: &str, value: &str, ty: u32) -> Result<(), String> {
        let w = wide(value);
        let bytes: Vec<u8> = w.iter().flat_map(|c| c.to_le_bytes()).collect();
        self.set(name, ty, &bytes)
    }

    pub fn set_str(&self, name: &str, value: &str) -> Result<(), String> {
        self.set_str_typed(name, value, REG_SZ)
    }

    pub fn set_dword(&self, name: &str, value: u32) -> Result<(), String> {
        self.set(name, REG_DWORD, &value.to_le_bytes())
    }

    pub fn delete_value(&self, name: &str) {
        let n = wide(name);
        // SAFETY: valid NUL-terminated name.
        unsafe { RegDeleteValueW(self.0, n.as_ptr()) };
    }

    fn is_empty(&self) -> bool {
        let (mut subkeys, mut values) = (0u32, 0u32);
        // SAFETY: only the two counts are requested; the rest are null.
        let r = unsafe {
            RegQueryInfoKeyW(
                self.0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null(),
                &mut subkeys,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut values,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        r == ERROR_SUCCESS && subkeys == 0 && values == 0
    }
}
