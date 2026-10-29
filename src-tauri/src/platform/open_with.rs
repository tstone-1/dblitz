//! The opt-in "Open with" entry for CSV and JSON files on Windows.
//!
//! The installer associates SQLite and Parquet files with dblitz, but not CSV
//! or JSON: an installer association writes `Software\Classes\.csv` itself,
//! which makes dblitz the double-click handler on every machine where the user
//! never chose one, and those files belong to spreadsheets and editors. This
//! setting writes only the per-user keys that add dblitz to Explorer's
//! "Open with" list:
//!
//! - `Software\Classes\dblitz.<ext>` - a ProgID with an icon and an open
//!   command pointing at the running executable;
//! - a value named `dblitz.<ext>` under `Software\Classes\.<ext>\OpenWithProgids`.
//!
//! The extension's default value is never touched, so the user's default app
//! stays the default. Making dblitz the default is Windows' own "Always use
//! this app". The registry is the setting's only record, so the portable exe
//! can turn it on as well, and the installed build's uninstaller removes the
//! same keys (`windows/hooks.nsh`, pinned by a test below).

use windows::core::PCWSTR;
use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, WIN32_ERROR};
use windows::Win32::System::Registry::{
    RegDeleteKeyValueW, RegDeleteTreeW, RegSetKeyValueW, HKEY, HKEY_CURRENT_USER, REG_SZ,
};

/// The extensions dblitz opens as CSV or JSON (`pq::FileFormat::of`).
pub(crate) const EXTENSIONS: [(&str, &str); 5] = [
    ("csv", "CSV file"),
    ("tsv", "TSV file"),
    ("json", "JSON file"),
    ("jsonl", "JSON Lines file"),
    ("ndjson", "NDJSON file"),
];

const CLASSES: &str = r"Software\Classes";

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn check(rc: WIN32_ERROR, what: &str) -> Result<(), String> {
    if rc == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(format!("{what}: Windows error {}", rc.0))
    }
}

/// Write one `REG_SZ` value; `RegSetKeyValueW` creates the key if needed.
fn set_sz(root: HKEY, subkey: &str, value: Option<&str>, data: &str) -> Result<(), String> {
    let (subkey_w, data_w) = (wide(subkey), wide(data));
    let value_w = value.map(wide);
    let rc = unsafe {
        RegSetKeyValueW(
            root,
            PCWSTR(subkey_w.as_ptr()),
            value_w
                .as_ref()
                .map_or(PCWSTR::null(), |v| PCWSTR(v.as_ptr())),
            REG_SZ.0,
            Some(data_w.as_ptr().cast()),
            (data_w.len() * 2) as u32,
        )
    };
    check(rc, subkey)
}

fn prog_id(ext: &str) -> String {
    format!("dblitz.{ext}")
}

fn open_command(exe: &str) -> String {
    format!("\"{exe}\" \"%1\"")
}

/// Whether every extension lists dblitz under "Open with", for any copy of it.
fn is_registered_in(root: HKEY, classes: &str) -> bool {
    EXTENSIONS.iter().all(|(ext, _)| {
        let id = prog_id(ext);
        super::windows::read_reg_sz(root, &format!(r"{classes}\.{ext}\OpenWithProgids"), &id)
            .is_some()
            && super::windows::read_reg_sz(root, &format!(r"{classes}\{id}\shell\open\command"), "")
                .is_some()
    })
}

fn register_in(root: HKEY, classes: &str, exe: &str) -> Result<(), String> {
    for (ext, description) in EXTENSIONS {
        let id = prog_id(ext);
        set_sz(root, &format!(r"{classes}\{id}"), None, description)?;
        set_sz(
            root,
            &format!(r"{classes}\{id}\DefaultIcon"),
            None,
            &format!("\"{exe}\",0"),
        )?;
        set_sz(
            root,
            &format!(r"{classes}\{id}\shell\open\command"),
            None,
            &open_command(exe),
        )?;
        set_sz(
            root,
            &format!(r"{classes}\.{ext}\OpenWithProgids"),
            Some(&id),
            "",
        )?;
    }
    Ok(())
}

/// Remove what `register_in` wrote. The `.<ext>\OpenWithProgids` key stays,
/// because other applications list themselves there too.
fn unregister_in(root: HKEY, classes: &str) -> Result<(), String> {
    let gone = |rc: WIN32_ERROR| {
        if rc == ERROR_FILE_NOT_FOUND {
            ERROR_SUCCESS
        } else {
            rc
        }
    };
    for (ext, _) in EXTENSIONS {
        let id = prog_id(ext);
        let list = wide(&format!(r"{classes}\.{ext}\OpenWithProgids"));
        let (id_w, key_w) = (wide(&id), wide(&format!(r"{classes}\{id}")));
        let rc = unsafe { RegDeleteKeyValueW(root, PCWSTR(list.as_ptr()), PCWSTR(id_w.as_ptr())) };
        check(gone(rc), &id)?;
        let rc = unsafe { RegDeleteTreeW(root, PCWSTR(key_w.as_ptr())) };
        check(gone(rc), &id)?;
    }
    Ok(())
}

pub(crate) fn is_registered() -> bool {
    is_registered_in(HKEY_CURRENT_USER, CLASSES)
}

/// Add or remove the entries, then tell Explorer the associations changed.
pub(crate) fn set_registered(enabled: bool) -> Result<(), String> {
    let result = if enabled {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        register_in(HKEY_CURRENT_USER, CLASSES, &exe.to_string_lossy())
    } else {
        unregister_in(HKEY_CURRENT_USER, CLASSES)
    };
    use windows::Win32::UI::Shell::{SHChangeNotify, SHCNE_ASSOCCHANGED, SHCNF_IDLIST};
    unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None) };
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// A private stand-in for `Software\Classes` under HKCU, deleted on drop,
    /// so the tests never touch the real associations.
    struct Scratch(String);

    impl Scratch {
        fn new() -> Self {
            static N: AtomicU32 = AtomicU32::new(0);
            let n = N.fetch_add(1, Ordering::Relaxed);
            Scratch(format!(r"Software\dblitz-test-{}-{n}", std::process::id()))
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let key = wide(&self.0);
            let _ = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, PCWSTR(key.as_ptr())) };
        }
    }

    fn read(classes: &str, subkey: &str, value: &str) -> Option<String> {
        super::super::windows::read_reg_sz(
            HKEY_CURRENT_USER,
            &format!(r"{classes}\{subkey}"),
            value,
        )
    }

    #[test]
    fn registering_adds_open_with_entries_and_leaves_the_default_alone() {
        let s = Scratch::new();
        // Another app owns .csv and lists itself under OpenWithProgids.
        set_sz(
            HKEY_CURRENT_USER,
            &format!(r"{}\.csv", s.0),
            None,
            "Excel.CSV",
        )
        .unwrap();
        set_sz(
            HKEY_CURRENT_USER,
            &format!(r"{}\.csv\OpenWithProgids", s.0),
            Some("Excel.CSV"),
            "",
        )
        .unwrap();
        assert!(!is_registered_in(HKEY_CURRENT_USER, &s.0));

        register_in(HKEY_CURRENT_USER, &s.0, r"C:\Apps\dblitz\dblitz.exe").unwrap();
        assert!(is_registered_in(HKEY_CURRENT_USER, &s.0));
        assert_eq!(read(&s.0, ".csv", "").as_deref(), Some("Excel.CSV"));
        for (ext, description) in EXTENSIONS {
            let id = prog_id(ext);
            assert_eq!(
                read(&s.0, &format!(r".{ext}\OpenWithProgids"), &id).as_deref(),
                Some("")
            );
            assert_eq!(read(&s.0, &id, "").as_deref(), Some(description));
            assert_eq!(
                read(&s.0, &format!(r"{id}\shell\open\command"), "").as_deref(),
                Some(r#""C:\Apps\dblitz\dblitz.exe" "%1""#)
            );
        }
    }

    #[test]
    fn unregistering_removes_only_dblitzs_entries() {
        let s = Scratch::new();
        set_sz(
            HKEY_CURRENT_USER,
            &format!(r"{}\.json\OpenWithProgids", s.0),
            Some("Code.json"),
            "",
        )
        .unwrap();
        register_in(HKEY_CURRENT_USER, &s.0, r"C:\x\dblitz.exe").unwrap();
        unregister_in(HKEY_CURRENT_USER, &s.0).unwrap();

        assert!(!is_registered_in(HKEY_CURRENT_USER, &s.0));
        for (ext, _) in EXTENSIONS {
            let id = prog_id(ext);
            assert_eq!(read(&s.0, &format!(r".{ext}\OpenWithProgids"), &id), None);
            assert_eq!(read(&s.0, &id, ""), None, "{id} key left behind");
        }
        assert_eq!(
            read(&s.0, r".json\OpenWithProgids", "Code.json").as_deref(),
            Some("")
        );
        // A second removal finds nothing and is not an error.
        unregister_in(HKEY_CURRENT_USER, &s.0).unwrap();
    }

    #[test]
    fn one_missing_extension_reads_as_not_registered() {
        let s = Scratch::new();
        register_in(HKEY_CURRENT_USER, &s.0, r"C:\x\dblitz.exe").unwrap();
        let (list, id) = (
            wide(&format!(r"{}\.ndjson\OpenWithProgids", s.0)),
            wide("dblitz.ndjson"),
        );
        let rc = unsafe {
            RegDeleteKeyValueW(
                HKEY_CURRENT_USER,
                PCWSTR(list.as_ptr()),
                PCWSTR(id.as_ptr()),
            )
        };
        assert_eq!(rc, ERROR_SUCCESS);
        assert!(!is_registered_in(HKEY_CURRENT_USER, &s.0));
    }

    /// The uninstaller removes the same entries: every ProgID here has its
    /// `DeleteRegValue` and `DeleteRegKey` line, and the hook names no other.
    #[test]
    fn the_uninstall_hook_removes_exactly_these_entries() {
        let hook = include_str!("../../windows/hooks.nsh");
        let lines: Vec<&str> = hook.lines().map(str::trim).collect();
        let mut named: Vec<&str> = hook
            .match_indices("dblitz.")
            .map(|(i, _)| &hook[i..])
            .collect();
        named
            .iter_mut()
            .for_each(|s| *s = &s[..s.find(['"', '\\']).unwrap_or(s.len())]);
        assert_eq!(named.len(), 2 * EXTENSIONS.len(), "{named:?}");
        for (ext, _) in EXTENSIONS {
            let id = prog_id(ext);
            let value =
                format!(r#"DeleteRegValue HKCU "Software\Classes\.{ext}\OpenWithProgids" "{id}""#);
            let key = format!(r#"DeleteRegKey HKCU "Software\Classes\{id}""#);
            assert!(lines.contains(&value.as_str()), "missing: {value}");
            assert!(lines.contains(&key.as_str()), "missing: {key}");
        }
        // Run only on a real uninstall: an update runs the old uninstaller
        // with /UPDATE first, and must keep the user's choice.
        assert!(hook.contains("${If} $UpdateMode <> 1"));
    }
}
