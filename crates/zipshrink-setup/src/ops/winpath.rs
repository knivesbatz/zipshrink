//! Pure helpers for Windows path strings: comparing folders the way Windows
//! does, and editing the user's `Path` variable. No system calls here, so
//! the tests run on every platform; `windows.rs` passes in the real
//! environment-variable expansion.

/// A folder or file path in a form two spellings of the same place share:
/// `%VARS%` expanded, quotes, surrounding spaces and trailing separators
/// removed, `/` turned into `\`, and lower case (Windows paths ignore case).
pub fn normalize(path: &str, expand: &dyn Fn(&str) -> String) -> String {
    let p = path.trim();
    let p = p.strip_prefix('"').and_then(|q| q.strip_suffix('"')).unwrap_or(p).trim();
    let p = if p.contains('%') { expand(p) } else { p.to_owned() };
    let p = p.replace('/', "\\");
    // "C:\" keeps its separator: "C:" alone means "the current folder on C:".
    let mut p = p.as_str();
    while p.len() > 3 && p.ends_with('\\') {
        p = &p[..p.len() - 1];
    }
    p.to_lowercase()
}

/// Whether `a` and `b` name the same place.
pub fn same(a: &str, b: &str, expand: &dyn Fn(&str) -> String) -> bool {
    normalize(a, expand) == normalize(b, expand)
}

/// Whether the `;`-separated `list` holds `dir`.
pub fn list_contains(list: &str, dir: &str, expand: &dyn Fn(&str) -> String) -> bool {
    let want = normalize(dir, expand);
    !want.is_empty() && list.split(';').any(|e| normalize(e, expand) == want)
}

/// `list` with `dir` added at the end, or with later duplicates of it
/// removed when it is already there. `None` when nothing needs to change.
/// Every other entry is kept exactly as written (including `%VARS%`).
pub fn list_add(list: &str, dir: &str, expand: &dyn Fn(&str) -> String) -> Option<String> {
    let want = normalize(dir, expand);
    if want.is_empty() {
        return None;
    }
    let mut seen = false;
    let mut changed = false;
    let mut kept: Vec<&str> = Vec::new();
    for e in list.split(';') {
        if normalize(e, expand) == want {
            if seen {
                changed = true;
                continue;
            }
            seen = true;
        }
        kept.push(e);
    }
    if seen {
        return changed.then(|| kept.join(";"));
    }
    let trimmed = list.trim_end_matches(';');
    Some(if trimmed.trim().is_empty() { dir.to_owned() } else { format!("{trimmed};{dir}") })
}

/// `list` without any entry naming `dir`; `None` when it has none.
pub fn list_remove(list: &str, dir: &str, expand: &dyn Fn(&str) -> String) -> Option<String> {
    let want = normalize(dir, expand);
    if want.is_empty() {
        return None;
    }
    let before = list.split(';').count();
    let kept: Vec<&str> = list.split(';').filter(|e| normalize(e, expand) != want).collect();
    (kept.len() != before).then(|| kept.join(";"))
}

/// Whether a right-click menu command (`"C:\dir\zipshrink-gui.exe" "%1"`)
/// starts the program `exe`.
pub fn command_runs(command: &str, exe: &str, expand: &dyn Fn(&str) -> String) -> bool {
    let c = command.trim();
    let program = match c.strip_prefix('"') {
        Some(rest) => rest.split('"').next().unwrap_or(""),
        None => c.split(' ').next().unwrap_or(""),
    };
    same(program, exe, expand)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(s: &str) -> String {
        s.replace("%LOCALAPPDATA%", r"C:\Users\Ann\AppData\Local").replace("%USERPROFILE%", r"C:\Users\Ann")
    }
    const DIR: &str = r"C:\Users\Ann\AppData\Local\Programs\ZipShrink";

    #[test]
    fn normalize_spellings() {
        for s in [
            DIR,
            r"c:\users\ann\appdata\local\programs\zipshrink\",
            r" C:/Users/Ann/AppData/Local/Programs/ZipShrink// ",
            r#""C:\Users\Ann\AppData\Local\Programs\ZipShrink""#,
            r"%LOCALAPPDATA%\Programs\ZipShrink",
        ] {
            assert!(same(s, DIR, &env), "{s}");
        }
        assert!(!same(r"C:\Users\Ann\AppData\Local\Programs\ZipShrink2", DIR, &env));
        assert_eq!(normalize(r"C:\", &env), r"c:\");
        assert_eq!(normalize(r"C:\\", &env), r"c:\");
    }

    #[test]
    fn add_to_empty_and_to_existing() {
        assert_eq!(list_add("", DIR, &env).as_deref(), Some(DIR));
        assert_eq!(list_add("  ", DIR, &env).as_deref(), Some(DIR));
        assert_eq!(list_add(";", DIR, &env).as_deref(), Some(DIR));
        let other = r"%USERPROFILE%\bin;C:\Tools";
        assert_eq!(list_add(other, DIR, &env), Some(format!("{other};{DIR}")));
        assert_eq!(list_add(&format!("{other};"), DIR, &env), Some(format!("{other};{DIR}")));
    }

    #[test]
    fn add_keeps_a_present_entry_and_dedupes() {
        let list = format!(r"C:\Tools;{}\;%USERPROFILE%\bin", DIR.to_uppercase());
        assert_eq!(list_add(&list, DIR, &env), None, "already there, in another spelling");
        let list = format!(r"{DIR};C:\Tools;%LOCALAPPDATA%\Programs\ZipShrink;{DIR}\");
        assert_eq!(list_add(&list, DIR, &env), Some(format!(r"{DIR};C:\Tools")));
        // A longer folder that merely starts the same is someone else's.
        let list = format!(r"{DIR}-old");
        assert_eq!(list_add(&list, DIR, &env), Some(format!(r"{DIR}-old;{DIR}")));
        assert!(!list_contains(&list, DIR, &env));
    }

    #[test]
    fn remove_every_spelling_and_nothing_else() {
        let list = format!(r"{DIR};C:\Tools;%LOCALAPPDATA%\Programs\ZipShrink\;%USERPROFILE%\bin;{DIR}-old");
        let out = list_remove(&list, DIR, &env).unwrap();
        assert_eq!(out, format!(r"C:\Tools;%USERPROFILE%\bin;{DIR}-old"));
        assert_eq!(list_remove(&out, DIR, &env), None);
        assert_eq!(list_remove(DIR, DIR, &env).as_deref(), Some(""));
        assert_eq!(list_remove("", DIR, &env), None);
        assert_eq!(list_remove(DIR, "", &env), None);
    }

    #[test]
    fn contains_and_round_trip() {
        let list = r"C:\Tools;;%USERPROFILE%\bin";
        assert!(!list_contains(list, DIR, &env));
        let added = list_add(list, DIR, &env).unwrap();
        assert!(list_contains(&added, DIR, &env));
        assert_eq!(list_remove(&added, DIR, &env).as_deref(), Some(list));
        assert!(!list_contains("", "", &env));
    }

    #[test]
    fn menu_command_program() {
        let exe = format!(r"{DIR}\zipshrink-gui.exe");
        assert!(command_runs(&format!(r#""{DIR}\zipshrink-gui.exe" "%1""#), &exe, &env));
        assert!(command_runs(&format!(r#""{}\ZIPSHRINK-GUI.EXE" "%1""#, DIR.to_lowercase()), &exe, &env));
        assert!(command_runs(r"C:\Z\zipshrink-gui.exe %1", r"C:\Z\zipshrink-gui.exe", &env));
        assert!(!command_runs(r#""C:\Other\zipshrink-gui.exe" "%1""#, &exe, &env));
        assert!(!command_runs("", &exe, &env));
    }
}
