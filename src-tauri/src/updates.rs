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
//!    install on Linux, where the Tauri updater supports AppImage only, and
//!    false for the portable `dblitz.exe` on Windows, which has no installer
//!    to hand off to. The UI uses it to report an available version *without*
//!    offering an Install button it cannot honour.
//!
//! Deliberately a pure module: no `tauri::` imports, no environment reads, no
//! `cfg!` branches, no registry access. Everything host-dependent is resolved
//! by the caller in `lib.rs` and arrives here as one [`InstallProvenance`]
//! value, so every platform's behaviour is unit-testable from any machine —
//! the Linux and Windows gates are both covered by tests running anywhere.

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

/// How this build was installed, as far as the updater is concerned.
///
/// Resolved by `lib.rs`, because answering it needs the target OS, the process
/// environment and — on Windows — the registry. This module stays pure and
/// just maps the answer onto a yes/no.
///
/// `allow(dead_code)`: every build constructs exactly one variant, so the other
/// four are unconstructed on any given target. Keeping the enum whole on all
/// platforms is the point of the module — it is what lets the Windows and Linux
/// gates be unit-tested from a Mac, which is the property these rules exist to
/// preserve (see the module docs). The tests below do construct all five.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallProvenance {
    /// macOS. The updater swaps the `.app` bundle in place, which always works
    /// regardless of how the bundle got there (DMG, Homebrew cask, or a manual
    /// copy into /Applications).
    MacOsBundle,
    /// Windows, running from the directory the NSIS installer registered as its
    /// `InstallLocation`. The updater hands the downloaded NSIS payload to that
    /// installer, which upgrades this copy.
    WindowsInstaller,
    /// Windows, running from anywhere else: the published portable
    /// `dblitz.exe`, or a copy of an installed one carried off somewhere.
    ///
    /// **Also what `lib.rs` reports when provenance cannot be established at
    /// all** — an unreadable or absent registry key lands here on purpose. The
    /// two failure directions are not symmetric: wrongly claiming portable
    /// costs an installed user one manual download, while wrongly claiming
    /// installed runs the NSIS installer, which cannot find the standalone exe
    /// to replace. That produces a *second*, installed copy at the new version
    /// while the file the user actually launches stays old — an update that
    /// reports success and changes nothing they can see.
    WindowsPortable,
    /// Linux AppImage, which identifies itself by exporting a non-empty
    /// `$APPIMAGE`. The one Linux format the Tauri updater can replace.
    LinuxAppImage,
    /// Linux `.deb`/`.rpm`: files owned by the package manager, under paths the
    /// updater must not touch. Also the answer for any Linux build with no
    /// `$APPIMAGE` to point at.
    LinuxPackage,
}

impl UpdateStatus {
    /// `provenance` is a parameter rather than being detected here so this
    /// stays pure and testable — see the module docs.
    pub fn new(
        previous_version: Option<String>,
        current_version: String,
        provenance: InstallProvenance,
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
            self_update_supported: self_update_supported(provenance),
        }
    }
}

/// Whether the Tauri updater can install over this running build.
///
/// Exhaustive over [`InstallProvenance`] on purpose: the question is "which
/// artifact is this?", and a new artifact must not silently inherit a default
/// answer. Two of the five cannot be replaced in place, for the same underlying
/// reason in different clothes — the updater hands its payload to an installer,
/// and neither a package-managed Linux install nor a standalone Windows exe has
/// one that owns this copy.
///
/// The consequence for the UI is not "hide the update" but "don't offer to
/// install it": a `.deb` or portable-exe user still wants to know a new version
/// exists, and gets pointed at the release page instead
/// (`AppBanners.svelte`).
fn self_update_supported(provenance: InstallProvenance) -> bool {
    match provenance {
        InstallProvenance::MacOsBundle
        | InstallProvenance::WindowsInstaller
        | InstallProvenance::LinuxAppImage => true,
        InstallProvenance::WindowsPortable | InstallProvenance::LinuxPackage => false,
    }
}

/// Classify a Linux build from its `$APPIMAGE`.
///
/// An empty value is treated exactly like an absent one: an exported but empty
/// variable gives the updater nothing to replace either. Lives here rather than
/// in `lib.rs` so the rule is tested with the rest of the policy — and stays
/// compiled on every target for the same reason, hence `allow(dead_code)`.
#[allow(dead_code)]
pub fn linux_provenance(appimage_env: Option<&str>) -> InstallProvenance {
    match appimage_env {
        Some(value) if !value.is_empty() => InstallProvenance::LinuxAppImage,
        _ => InstallProvenance::LinuxPackage,
    }
}

#[cfg(test)]
mod tests {
    use super::{linux_provenance, self_update_supported, InstallProvenance, UpdateStatus};

    /// macOS by default: these cases are about the version transition only.
    fn status(previous: Option<&str>, current: &str) -> UpdateStatus {
        UpdateStatus::new(
            previous.map(str::to_string),
            current.to_string(),
            InstallProvenance::MacOsBundle,
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
    fn macos_can_always_self_update() {
        // The .app swap works however the bundle got there.
        assert!(self_update_supported(InstallProvenance::MacOsBundle));
    }

    #[test]
    fn installed_windows_build_can_self_update() {
        assert!(self_update_supported(InstallProvenance::WindowsInstaller));
    }

    #[test]
    fn portable_windows_build_does_not_offer_install() {
        // The published portable dblitz.exe has no installer that owns it. The
        // updater would run the NSIS payload anyway, producing a second,
        // installed copy at the new version while the exe the user launches
        // stays old -- so the UI must not offer the button at all. README.md's
        // "Updates" section states this as a promise to the user.
        assert!(!self_update_supported(InstallProvenance::WindowsPortable));
    }

    #[test]
    fn linux_appimage_can_self_update() {
        assert!(self_update_supported(InstallProvenance::LinuxAppImage));
    }

    #[test]
    fn linux_deb_or_rpm_cannot_self_update() {
        // A package-managed install the updater must not touch.
        assert!(!self_update_supported(InstallProvenance::LinuxPackage));
    }

    #[test]
    fn appimage_env_classifies_the_linux_artifact() {
        assert_eq!(
            linux_provenance(Some("/tmp/dblitz.AppImage")),
            InstallProvenance::LinuxAppImage
        );
        assert_eq!(linux_provenance(None), InstallProvenance::LinuxPackage);
        // Exported but empty gives the plugin no bundle to replace, so it is
        // treated exactly like absent rather than as a truthy AppImage.
        assert_eq!(linux_provenance(Some("")), InstallProvenance::LinuxPackage);
    }

    #[test]
    fn capability_reaches_the_status_payload() {
        for provenance in [
            InstallProvenance::LinuxPackage,
            InstallProvenance::WindowsPortable,
        ] {
            let status = UpdateStatus::new(None, "26.7.6".to_string(), provenance);
            assert!(!status.self_update_supported, "{provenance:?}");
        }
        for provenance in [
            InstallProvenance::MacOsBundle,
            InstallProvenance::WindowsInstaller,
            InstallProvenance::LinuxAppImage,
        ] {
            let status = UpdateStatus::new(None, "26.7.6".to_string(), provenance);
            assert!(status.self_update_supported, "{provenance:?}");
        }
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
