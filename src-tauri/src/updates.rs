//! Version-transition and self-update-capability reporting for the in-app updater.
//!
//! The updater itself lives in `tauri-plugin-updater` and is driven from the
//! frontend. Rust owns only two questions, both answered once at startup and
//! handed to the UI as [`UpdateStatus`]:
//!
//! 1. **"Is this launch the first one after an update?"** — used to show a
//!    one-time "dblitz was updated to vX" notice, which is the only feedback a
//!    user gets that the silent download+relaunch actually worked.
//! 2. **"Can this install replace itself at all?"** — false for a `.deb`/`.rpm`
//!    install on Linux, where the Tauri updater supports AppImage only. The UI
//!    uses it to report an available version *without* offering an Install
//!    button it cannot honour.
//!
//! Deliberately a pure module: no `tauri::` imports, no environment reads, no
//! `cfg!` branches. Both inputs that would otherwise make it host-dependent
//! (the OS, and `$APPIMAGE`) are passed in by the caller in `lib.rs`, so every
//! platform's behaviour is unit-testable from any machine — the Linux gate is
//! covered by tests running on macOS and Windows.

use serde::{Deserialize, Serialize};

/// Everything the frontend needs to know about this launch's version state.
/// Resolved once during `setup` and handed out unchanged (see `lib.rs`), so the
/// "did we just update?" answer survives `app.json` having already been
/// rewritten with the current version.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UpdateStatus {
    /// The build that ran last, or `None` on a first run.
    pub previous_version: Option<String>,
    /// The build running now.
    pub current_version: String,
    /// Whether this launch is the first after a version change. Computed here
    /// rather than re-derived in TypeScript so the "a first run is not an
    /// update" rule has exactly one implementation, tested once.
    pub updated: bool,
    /// Whether this install can replace itself in place. See
    /// [`self_update_supported`].
    pub self_update_supported: bool,
}

impl UpdateStatus {
    /// `is_linux` and `appimage_env` are parameters rather than being read here
    /// so this stays pure and testable — see the module docs.
    pub fn new(
        previous_version: Option<String>,
        current_version: String,
        is_linux: bool,
        appimage_env: Option<&str>,
    ) -> Self {
        // `updated` is true only when a *different* version ran before this
        // one. A first run (`None`) is deliberately not an update: there is
        // nothing to confirm, and claiming otherwise would greet every new user
        // with a notice about an update that never happened.
        let updated = matches!(&previous_version, Some(previous) if previous != &current_version);
        Self {
            previous_version,
            current_version,
            updated,
            self_update_supported: self_update_supported(is_linux, appimage_env),
        }
    }
}

/// Whether the Tauri updater can install over this running build.
///
/// Windows (NSIS) and macOS (in-place `.app` swap) always can. On Linux the
/// updater supports **AppImage only** — a `.deb` or `.rpm` install lives under
/// paths owned by the package manager, and the plugin refuses to touch it. An
/// AppImage identifies itself by exporting `$APPIMAGE`, which is therefore the
/// signal used here; an empty value is treated as absent because an exported
/// but empty variable gives the plugin nothing to replace either.
///
/// The consequence for the UI is not "hide the update" but "don't offer to
/// install it": a `.deb` user still wants to know a new version exists, and
/// gets pointed at the release page instead.
fn self_update_supported(is_linux: bool, appimage_env: Option<&str>) -> bool {
    !is_linux || appimage_env.is_some_and(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::{self_update_supported, UpdateStatus};

    /// Non-Linux by default: these cases are about the version transition only.
    fn status(previous: Option<&str>, current: &str) -> UpdateStatus {
        UpdateStatus::new(
            previous.map(str::to_string),
            current.to_string(),
            false,
            None,
        )
    }

    #[test]
    fn first_run_is_not_an_update() {
        assert!(!status(None, "26.7.6").updated);
    }

    #[test]
    fn same_version_relaunch_is_not_an_update() {
        assert!(!status(Some("26.7.6"), "26.7.6").updated);
    }

    #[test]
    fn changed_version_is_an_update() {
        assert!(status(Some("26.7.5"), "26.7.6").updated);
    }

    #[test]
    fn downgrade_counts_as_an_update() {
        // A sideways or backwards move is still a version change, so the
        // "updated to X" notice is still the truthful thing to show.
        assert!(status(Some("26.7.6"), "26.7.5").updated);
    }

    #[test]
    fn previous_and_current_are_reported_verbatim() {
        let status = status(Some("26.7.5"), "26.7.6");
        assert_eq!(status.previous_version.as_deref(), Some("26.7.5"));
        assert_eq!(status.current_version, "26.7.6");
    }

    #[test]
    fn windows_and_macos_can_always_self_update() {
        // No AppImage variable exists off Linux; it must not gate anything there.
        assert!(self_update_supported(false, None));
    }

    #[test]
    fn linux_appimage_can_self_update() {
        assert!(self_update_supported(true, Some("/tmp/dblitz.AppImage")));
    }

    #[test]
    fn linux_deb_or_rpm_cannot_self_update() {
        // No $APPIMAGE: a package-managed install the updater must not touch.
        assert!(!self_update_supported(true, None));
    }

    #[test]
    fn linux_empty_appimage_var_cannot_self_update() {
        // Exported but empty gives the plugin no bundle to replace, so it is
        // treated exactly like absent rather than as a truthy AppImage.
        assert!(!self_update_supported(true, Some("")));
    }

    #[test]
    fn capability_reaches_the_status_payload() {
        let deb = UpdateStatus::new(None, "26.7.6".to_string(), true, None);
        assert!(!deb.self_update_supported);
        let appimage = UpdateStatus::new(None, "26.7.6".to_string(), true, Some("/tmp/x.AppImage"));
        assert!(appimage.self_update_supported);
    }

    #[test]
    fn serializes_camel_case_for_the_frontend() {
        // The frontend reads these keys by name (src/lib/ipc.ts), so the
        // rename_all is load-bearing, not cosmetic.
        let json = serde_json::to_string(&status(Some("26.7.5"), "26.7.6")).unwrap();
        assert!(json.contains("\"previousVersion\""));
        assert!(json.contains("\"currentVersion\""));
        assert!(json.contains("\"selfUpdateSupported\""));
    }
}
