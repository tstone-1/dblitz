//! Windows-only glue: the duplicate-instance marker and lookup, install
//! provenance from the NSIS registry key, the AppUserModelID and the jump list.
//! The whole module is `cfg(windows)` (see `platform/mod.rs`).

use crate::updates::InstallProvenance;
use tauri::{AppHandle, Manager};

/// Compute a 64-bit hash of the lowercased path for cross-process duplicate
/// detection via Win32 window properties.
///
/// `DefaultHasher::new` is seeded with fixed keys (unlike `RandomState`), so the
/// value is stable across processes — which is the whole point, since one
/// instance writes it and another reads it. Rust does not promise the algorithm
/// is stable across *compiler* versions, so two dblitz builds from different
/// toolchains may not recognise each other; that degrades to opening a second
/// window, never to opening the wrong one.
fn path_hash(path: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    path.to_ascii_lowercase().hash(&mut hasher);
    hasher.finish()
}

/// Set the Win32 window property (`dblitz_db_path`) containing a hash of the
/// full path, used by [`try_activate_existing`] for duplicate detection.
/// Kept separate from [`update_window_title`](crate::commands::update_window_title): the title is purely cosmetic,
/// but this marker is load-bearing state another process reads to find this
/// window, so callers that change which file is open must always set both
/// together rather than risk a title-only path leaving it stale.
pub(crate) fn set_window_db_marker(app: &AppHandle, path: Option<&str>) {
    if let Some(window) = app.get_webview_window("main") {
        if let Ok(hwnd) = window.hwnd() {
            use windows::core::w;
            use windows::Win32::Foundation::{HANDLE, HWND};
            use windows::Win32::UI::WindowsAndMessaging::SetPropW;
            // Tauri's windows-rs is pinned a version behind ours, so its
            // HWND type is structurally identical but type-distinct. Both
            // versions define HWND as `struct HWND(pub *mut c_void)`, so a
            // pointer-equivalent rewrap is sound.
            let hwnd = HWND(hwnd.0);
            let hash = path.map(path_hash).unwrap_or(0);
            unsafe {
                let _ = SetPropW(hwnd, w!("dblitz_db_path"), Some(HANDLE(hash as *mut _)));
            }
        }
    }
}

/// The registry key Tauri's NSIS installer writes on install; the trailing
/// segment is `productName` from `tauri.conf.json`. A test below pins the two
/// together, because a rename there would silently turn every installed copy
/// into a "portable" one that never offers an update again.
const NSIS_UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\dblitz";

/// Read one `REG_SZ` value, or `None` if the key or value is absent, is of
/// another type, or cannot be read at all. Every failure collapses to the same
/// answer on purpose - the caller's fallback is the safe one, so there is
/// nothing useful to distinguish.
pub(super) fn read_reg_sz(
    root: windows::Win32::System::Registry::HKEY,
    subkey: &str,
    value: &str,
) -> Option<String> {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::ERROR_SUCCESS;
    use windows::Win32::System::Registry::{RegGetValueW, RRF_RT_REG_SZ};

    let wide = |s: &str| -> Vec<u16> { s.encode_utf16().chain(std::iter::once(0)).collect() };
    let subkey = wide(subkey);
    let value = wide(value);

    // First call sizes the buffer, second fills it. `RegGetValueW` guarantees
    // the returned REG_SZ is NUL-terminated even when the stored value is not,
    // which is the reason to prefer it over `RegQueryValueExW` here.
    let mut size: u32 = 0;
    let rc = unsafe {
        RegGetValueW(
            root,
            PCWSTR(subkey.as_ptr()),
            PCWSTR(value.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            None,
            Some(&mut size),
        )
    };
    if rc != ERROR_SUCCESS || size == 0 {
        return None;
    }

    let mut buf = vec![0u16; size.div_ceil(2) as usize];
    let rc = unsafe {
        RegGetValueW(
            root,
            PCWSTR(subkey.as_ptr()),
            PCWSTR(value.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut size),
        )
    };
    if rc != ERROR_SUCCESS {
        return None;
    }

    // `size` is now the byte count INCLUDING the terminating NUL.
    let chars = (size as usize / 2).saturating_sub(1);
    Some(String::from_utf16_lossy(&buf[..chars.min(buf.len())]))
}

/// Whether this Windows build is the copy the NSIS installer registered, or a
/// standalone `dblitz.exe`.
///
/// Decided by comparing the running executable's directory against the
/// `InstallLocation` the installer recorded - checked under both HKCU (a
/// per-user install) and HKLM (per-machine), because the installer writes to
/// whichever context it ran in. Path identity is the test rather than the mere
/// existence of the key: a user can have dblitz installed *and* be running a
/// portable copy out of their Downloads folder, and only the running copy's
/// location answers which one this process is.
///
/// Anything that goes wrong - no key, unreadable registry, a recorded path that
/// no longer resolves - reports [`InstallProvenance::WindowsPortable`]. See
/// that variant's docs for why that is the safe direction.
pub(crate) fn windows_install_provenance() -> InstallProvenance {
    use windows::Win32::System::Registry::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};

    let Ok(exe) = std::env::current_exe() else {
        return InstallProvenance::WindowsPortable;
    };
    let Some(exe_dir) = exe.parent() else {
        return InstallProvenance::WindowsPortable;
    };
    // Canonicalise both sides so casing, 8.3 short names or a symlink in the
    // path cannot make two spellings of one directory compare unequal.
    let Ok(exe_dir) = exe_dir.canonicalize() else {
        return InstallProvenance::WindowsPortable;
    };

    for root in [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE] {
        let Some(raw) = read_reg_sz(root, NSIS_UNINSTALL_KEY, "InstallLocation") else {
            continue;
        };
        // The NSIS template writes this value WITH surrounding quotes, so an
        // un-trimmed compare never matches anything.
        let trimmed = raw.trim().trim_matches('"');
        if trimmed.is_empty() {
            continue;
        }
        let Ok(registered) = std::path::Path::new(trimmed).canonicalize() else {
            continue;
        };
        if registered == exe_dir {
            return InstallProvenance::WindowsInstaller;
        }
    }

    InstallProvenance::WindowsPortable
}

pub(crate) fn set_app_user_model_id() {
    use windows::core::w;
    use windows::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID;
    unsafe {
        let _ = SetCurrentProcessExplicitAppUserModelID(w!("com.tstone.dblitz"));
    }
}

pub(crate) fn add_to_recent_docs(path: &str) {
    use windows::Win32::UI::Shell::{SHAddToRecentDocs, SHARD_PATHW};
    let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        SHAddToRecentDocs(SHARD_PATHW.0 as u32, Some(wide.as_ptr() as *const _));
    }
}

/// Search for an existing dblitz window that has the same file open by
/// comparing the `dblitz_db_path` window property.
///
/// The property does not hold the path: it holds the 64-bit
/// [`path_hash`] of the case-folded path, written by [`set_window_db_marker`]
/// (`SetPropW` stores one pointer-sized value, which is why it is a hash and not
/// the string). So `path` here must be spelled the same way the other instance
/// spelled it - absolute - or the two hashes cannot agree; `run()` absolutizes
/// before calling. Case folding means `C:\Db\x.sqlite` and `c:\db\x.sqlite`
/// match, which is right on Windows; two genuinely different files colliding on
/// a 64-bit hash would raise the wrong window, which is why the value is a hash
/// of the whole path rather than of the filename.
///
/// If found, restore (un-minimise) and surface it, returning `true` so
/// the caller can exit early.
///
/// There is a narrow race between when an instance launches and when it
/// finishes loading its database (at which point the property is set). If the
/// same file is double-clicked twice within milliseconds the second instance
/// may not find the first. Acceptable in practice.
pub(crate) fn try_activate_existing(path: &str) -> bool {
    use windows::core::{w, BOOL};
    use windows::Win32::Foundation::{HWND, LPARAM};
    use windows::Win32::UI::WindowsAndMessaging::*;

    struct Ctx {
        target_hash: u64,
        found: HWND,
    }

    unsafe extern "system" fn cb(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let ctx = &mut *(lparam.0 as *mut Ctx);
        let prop = GetPropW(hwnd, w!("dblitz_db_path"));
        if !prop.is_invalid() && prop.0 as u64 == ctx.target_hash {
            ctx.found = hwnd;
            return BOOL(0); // stop enumerating
        }
        BOOL(1)
    }

    let mut ctx = Ctx {
        target_hash: path_hash(path),
        found: HWND::default(),
    };

    unsafe {
        let _ = EnumWindows(Some(cb), LPARAM(&mut ctx as *mut _ as isize));
        if ctx.found.0.is_null() {
            return false;
        }
        let hwnd = ctx.found;
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        }
        let _ = SetForegroundWindow(hwnd);
        // `SetForegroundWindow` is a request, not a command: Windows' foreground
        // lock denies it whenever the caller doesn't already own the foreground,
        // which is the normal case here — we were launched by Explorer, or a
        // full-screen app has focus. Measured on this machine with a full-screen
        // game focused: the call returned and the foreground never moved, so the
        // second launch exited 0 with no window, no message, nothing. That reads
        // as "dblitz refuses to open this file".
        //
        // So always follow up with a flash. It is deliberately unconditional:
        // there is no reliable way to ask whether the activation took.
        // `SetForegroundWindow`'s return value can be nonzero while it only
        // flashed the taskbar, and testing `GetForegroundWindow() != hwnd` right
        // after is racy in the other direction — measured reporting the *old*
        // foreground immediately after an activation that did succeed, which
        // would make the failure branch fire on the happy path. FLASHW_TIMERNOFG
        // makes the unconditional call self-correcting: it flashes only until
        // the window reaches the foreground, so a successful activation stops it
        // on its own and a denied one keeps flashing until the user looks.
        let info = FLASHWINFO {
            cbSize: std::mem::size_of::<FLASHWINFO>() as u32,
            hwnd,
            dwFlags: FLASHW_ALL | FLASHW_TIMERNOFG,
            uCount: 0,
            dwTimeout: 0,
        };
        let _ = FlashWindowEx(&info);
        eprintln!("dblitz: {path} is already open; raised the existing window");
        true
    }
}

#[cfg(test)]
mod tests {
    use super::{path_hash, NSIS_UNINSTALL_KEY};

    /// The uninstall key's last segment is `productName`, and NOTHING at build
    /// time couples the two. Rename the product in `tauri.conf.json` and the
    /// installer writes a key this lookup never finds, so every installed copy
    /// silently reclassifies itself as portable and stops offering updates -
    /// with no error anywhere, on exactly the machines least able to report it.
    #[test]
    fn uninstall_key_tracks_the_bundle_product_name() {
        let conf: serde_json::Value =
            serde_json::from_str(include_str!("../../tauri.conf.json")).unwrap();
        let product = conf["productName"].as_str().expect("productName");
        assert_eq!(
            NSIS_UNINSTALL_KEY.rsplit('\\').next(),
            Some(product),
            "NSIS_UNINSTALL_KEY must end with tauri.conf.json's productName"
        );
    }

    /// Explorer, the recent-files list and the file dialog can all hand back
    /// the same file with different casing, and NTFS treats those as one file.
    /// If the hash disagreed, double-clicking a database already open under a
    /// different casing would open a second window onto the same snapshot.
    #[test]
    fn path_hash_ignores_case() {
        assert_eq!(
            path_hash(r"C:\Users\alice\Data\inventory.sqlite"),
            path_hash(r"c:\users\ALICE\data\INVENTORY.SQLITE")
        );
    }

    /// The marker is keyed on the *full* path, deliberately: same-named
    /// databases in different directories are different files and must each get
    /// their own window. Hashing only the filename would collapse them.
    #[test]
    fn path_hash_distinguishes_directories() {
        assert_ne!(
            path_hash(r"C:\a\inventory.sqlite"),
            path_hash(r"C:\b\inventory.sqlite")
        );
    }

    /// Two different files must not collide into "already open" — that would
    /// make the second one silently un-openable.
    #[test]
    fn path_hash_distinguishes_filenames() {
        assert_ne!(path_hash(r"C:\a\one.sqlite"), path_hash(r"C:\a\two.sqlite"));
    }

    /// A `HANDLE` of 0 is what [`set_window_db_marker`] writes for "no file
    /// open", and `GetPropW` returns 0 for a window that has no such property
    /// at all. A path hashing to 0 would therefore match every non-dblitz
    /// window on the desktop. Not provable in general, but pin the paths this
    /// app actually sees so a hash swap that lands on 0 for a plausible input
    /// is caught here rather than in the wild.
    #[test]
    fn path_hash_is_nonzero_for_real_paths() {
        for p in [
            r"C:\db.sqlite",
            r"C:\Users\alice\Data\inventory.sqlite",
            r"\\server\share\db.sqlite",
            "/Users/alice/db.sqlite",
        ] {
            assert_ne!(path_hash(p), 0, "{p} hashed to the 'no file open' marker");
        }
    }
}
