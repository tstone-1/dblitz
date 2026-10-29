//! Which database to open at startup: the launch argument and OS
//! document-open requests, and the handoff to a webview that may not be
//! listening yet. Compiled on every platform, although only macOS sends
//! document-open events, so all of it stays under `cargo test` everywhere.

use tauri::{AppHandle, Manager};
use tracing::warn;

/// Slot for a database path the OS asked us to open before the webview was in
/// any position to hear about it.
///
/// macOS never puts a document open in argv: Finder double-clicks, Dock "Open
/// Recent" entries and `open -a dblitz file.db` all arrive as Apple events,
/// surfaced by Tauri as `RunEvent::Opened`. Those can land at any point in the
/// process lifetime, including before the webview has loaded — and an emitted
/// event with no listener is not queued, it is dropped, which reads to the user
/// as "double-clicking a database opens an empty dblitz".
///
/// So the handler picks exactly one of two delivery routes, and `webview_ready`
/// decides which:
///   - not ready yet: stash the path here; [`get_initial_file`](crate::commands::get_initial_file) hands it over
///     when the page mounts.
///   - ready: emit `open-file`, which the page is listening for by then.
///
/// Choosing one route rather than doing both is what stops a path that arrives
/// mid-mount from being opened twice.
#[derive(Default)]
pub(crate) struct PendingOpen(parking_lot::Mutex<PendingOpenState>);

#[derive(Default)]
struct PendingOpenState {
    path: Option<String>,
    /// Set by [`get_initial_file`](crate::commands::get_initial_file), which the page calls *after* registering its
    /// `open-file` listener — so this is a proxy for "an emit would be heard".
    webview_ready: bool,
}

impl PendingOpen {
    /// Record an OS open request. Returns `Some(path)` when the caller should
    /// emit it to the webview, `None` when it has been stashed instead.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    fn deliver(&self, path: String) -> Option<String> {
        let mut state = self.0.lock();
        if state.webview_ready {
            Some(path)
        } else {
            state.path = Some(path);
            None
        }
    }

    /// Drain the stash and mark the webview ready, falling back to the launch
    /// argument. `argv_path` is passed in rather than read here so the decision
    /// is testable without touching the process environment.
    pub(crate) fn take_initial(&self, argv_path: Option<String>) -> Option<String> {
        let mut state = self.0.lock();
        state.webview_ready = true;
        state.path.take().or(argv_path)
    }
}

/// Convert a `file://` URL as delivered by the OS into a plain filesystem path.
///
/// `RunEvent::Opened` carries URLs, not paths, so a database in
/// `~/My Databases/` arrives as `file:///Users/x/My%20Databases/db.sqlite`.
/// Handing that string to SQLite would look for a file whose name literally
/// contains "%20". Returns `None` for anything that is not a local file URL (a
/// custom scheme, a remote authority) or that is malformed, so the caller can
/// log and skip instead of opening something unintended.
///
/// Compiled on every platform even though only the macOS arm of the run-event
/// handler calls it: that keeps it under `cargo test` on the machines this is
/// actually developed on.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn file_url_to_path(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    if !scheme.eq_ignore_ascii_case("file") {
        return None;
    }
    // "file:///path" leaves the authority empty; "file://localhost/path" is the
    // one non-empty authority RFC 8089 still defines as meaning this machine.
    // Anything else names a remote host and is not ours to open.
    let slash = rest.find('/')?;
    let (authority, path) = rest.split_at(slash);
    if !(authority.is_empty() || authority.eq_ignore_ascii_case("localhost")) {
        return None;
    }
    // An unencoded '?' or '#' is a URL delimiter, not part of the path — a real
    // one in a filename would have arrived percent-encoded.
    let path = path.split(['?', '#']).next()?;
    let decoded = percent_decode(path)?;
    // An embedded NUL would be silently truncated by the C APIs downstream,
    // turning "open A\0B" into "open A".
    if decoded.is_empty() || decoded.contains('\0') {
        return None;
    }
    Some(decoded)
}

/// Percent-decode a URL path. `None` on a malformed escape or on bytes that do
/// not form valid UTF-8 — both mean we cannot tell which file was meant, which
/// is worth reporting rather than guessing at.
///
/// Deliberately does *not* treat '+' as a space: that is form encoding, and a
/// filename may legitimately contain a plus.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn percent_decode(input: &str) -> Option<String> {
    fn hex(byte: u8) -> Option<u8> {
        match byte {
            b'0'..=b'9' => Some(byte - b'0'),
            b'a'..=b'f' => Some(byte - b'a' + 10),
            b'A'..=b'F' => Some(byte - b'A' + 10),
            _ => None,
        }
    }
    let bytes = input.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hi = hex(*bytes.get(i + 1)?)?;
            let lo = hex(*bytes.get(i + 2)?)?;
            out.push((hi << 4) | lo);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Route one OS document-open request to whichever half of [`PendingOpen`] can
/// actually reach the user: emit it if the webview is listening, stash it if it
/// is not. A URL that is not a local file is logged and dropped.
///
/// Split out of the `RunEvent::Opened` arm rather than written inline so that it
/// compiles on Windows and Linux too: the `cfg` gate up there is unavoidable
/// (Tauri gates the variant itself), and everything left inside it is one
/// destructuring line. This is the part with types to get wrong, and it is
/// type-checked by every platform's build instead of only by the one nobody
/// develops on.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn handle_open_request(app: &AppHandle, url: &str) {
    use tauri::Emitter;
    let Some(path) = file_url_to_path(url) else {
        warn!(url, "Ignoring an open request that is not a local file URL");
        return;
    };
    let Some(path) = app.state::<PendingOpen>().deliver(path) else {
        return;
    };
    if let Err(e) = app.emit("open-file", path) {
        warn!(error = %e, "Failed to hand an open request to the webview");
    }
}

/// The database named on the command line, resolved to an absolute path.
///
/// One function, because there are two callers and they must agree: the webview
/// asks for it through `get_initial_file`, and the Windows duplicate-instance
/// check in `run()` hashes it to find an existing window. They did NOT agree -
/// `run()` read `argv[1]` raw - so the two spellings hashed differently and
/// `dblitz inventory.db` from the file's own directory opened a second window on
/// a file already open.
pub(crate) fn launch_argument() -> Option<String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    resolve_launch_argument(&args, |path| std::path::Path::new(path).is_file())
}

/// The half of [`launch_argument`] that does not read the process environment,
/// so it can be tested. `args` excludes the program name.
///
/// A path containing spaces can arrive split into several arguments, and the
/// in-app updater is what does it on Windows: Tauri's NSIS template relaunches
/// the updated app with `${GetOptions} $CMDLINE "/ARGS" $R0` followed by
/// `RunAsUser "...\dblitz.exe" "$R0"`. `GetOptions` strips the quotes the
/// updater plugin put around the path, so `...\Shared Files - Archive\x.sqlite`
/// came back as `...\Shared`, `Files`, `-`, `Archive\x.sqlite`, and dblitz
/// opened the directory `...\Shared`. When the first argument is not a file but all
/// of them joined by single spaces are, the joined path wins. A path holding a
/// run of two or more spaces cannot be recovered this way: the split already
/// lost how many there were.
fn resolve_launch_argument(args: &[String], is_file: impl Fn(&str) -> bool) -> Option<String> {
    let first = absolutize_launch_path(args.first()?);
    if args.len() > 1 && !is_file(&first) {
        let joined = absolutize_launch_path(&args.join(" "));
        if is_file(&joined) {
            return Some(joined);
        }
    }
    Some(first)
}

/// Resolve a launch-argument path against the process working directory.
///
/// `argv[1]` is the one path that reaches dblitz relative. Every other entry
/// point - the file dialog, drag-and-drop, the recents list, macOS document-open
/// Apple events - hands over an absolute path already. A relative one was passed
/// through untouched, and `path_to_sqlite_uri` then emitted
/// `file:/inventory.sqlite?immutable=1`, which names `/inventory.sqlite` at the
/// FILESYSTEM ROOT rather than the file in the shell's directory. So
/// `dblitz inventory.sqlite` either failed or, worse, opened an unrelated
/// root-level file of the same name.
///
/// Resolved here rather than inside `path_to_sqlite_uri` on purpose: the
/// absolute path is also what gets stored in the recents list, what the
/// per-database config key is derived from, and what the Windows
/// duplicate-instance check compares, and all four want one stable spelling of
/// the file. That was a statement of intent rather than of fact until this was
/// fixed - `run()` passed the raw `argv[1]` to `try_activate_existing`, so the marker
/// written from the absolute path and the hash looked up from the relative one
/// could not match. `std::path::absolute`
/// touches no filesystem (no existence check, no symlink resolution) and handles
/// the Windows drive-relative form `C:file.db`, which `cwd.join()` does not.
fn absolutize_launch_path(path: &str) -> String {
    if std::path::Path::new(path).is_absolute() {
        return path.to_string();
    }
    match std::path::absolute(path) {
        Ok(absolute) => absolute.to_string_lossy().into_owned(),
        // No working directory (deleted, or unreadable). Nothing better to say
        // than what the user typed; the open then fails with a real SQLite
        // error instead of silently reading a different file.
        Err(_) => path.to_string(),
    }
}

/// Everything about the OS "open this document" path that can be checked
/// without an OS: the URL-to-path conversion and the two-route delivery
/// decision. The event plumbing itself (`RunEvent::Opened` firing at all, the
/// webview receiving `open-file`) is macOS-only and is NOT covered here — see
/// the comments in [`PendingOpen`] and the run-event handler.
#[cfg(test)]
mod open_request_tests {
    use super::{absolutize_launch_path, file_url_to_path, resolve_launch_argument, PendingOpen};

    #[test]
    fn file_url_percent_escapes_are_decoded() {
        // The case that actually bites: a space in a folder name. Left encoded,
        // SQLite looks for a file whose name contains a literal "%20".
        assert_eq!(
            file_url_to_path("file:///Users/alice/My%20Databases/db.sqlite").as_deref(),
            Some("/Users/alice/My Databases/db.sqlite")
        );
        // Multi-byte UTF-8 arrives as one escape per byte.
        assert_eq!(
            file_url_to_path("file:///Users/alice/Gr%C3%B6%C3%9Fe.sqlite").as_deref(),
            Some("/Users/alice/Größe.sqlite")
        );
        // Uppercase hex digits are equally legal.
        assert_eq!(
            file_url_to_path("file:///tmp/a%2Db.sqlite").as_deref(),
            Some("/tmp/a-b.sqlite")
        );
    }

    #[test]
    fn file_url_without_escapes_passes_through() {
        assert_eq!(
            file_url_to_path("file:///Users/alice/db.sqlite").as_deref(),
            Some("/Users/alice/db.sqlite")
        );
    }

    #[test]
    fn plus_is_not_decoded_as_a_space() {
        // '+' means space in form encoding only. A filename may contain one,
        // and rewriting it produces a path that does not exist.
        assert_eq!(
            file_url_to_path("file:///tmp/c++notes.sqlite").as_deref(),
            Some("/tmp/c++notes.sqlite")
        );
    }

    #[test]
    fn localhost_authority_is_local_but_a_remote_host_is_not() {
        assert_eq!(
            file_url_to_path("file://localhost/Users/alice/db.sqlite").as_deref(),
            Some("/Users/alice/db.sqlite")
        );
        assert_eq!(file_url_to_path("file://server/share/db.sqlite"), None);
    }

    #[test]
    fn non_file_urls_are_rejected() {
        // The handler skips these rather than opening something unintended;
        // returning a path here would mean handing SQLite an https URL.
        assert_eq!(file_url_to_path("https://example.com/db.sqlite"), None);
        assert_eq!(file_url_to_path("dblitz://open/db.sqlite"), None);
        assert_eq!(file_url_to_path("/Users/alice/db.sqlite"), None);
    }

    #[test]
    fn scheme_matching_is_case_insensitive() {
        assert_eq!(
            file_url_to_path("FILE:///Users/alice/db.sqlite").as_deref(),
            Some("/Users/alice/db.sqlite")
        );
    }

    #[test]
    fn malformed_or_unopenable_urls_are_rejected() {
        // A truncated escape and a non-hex escape: we cannot tell what file was
        // meant, so say so instead of guessing.
        assert_eq!(file_url_to_path("file:///tmp/a%2"), None);
        assert_eq!(file_url_to_path("file:///tmp/a%zz.sqlite"), None);
        // Invalid UTF-8 (a lone continuation byte).
        assert_eq!(file_url_to_path("file:///tmp/%FF.sqlite"), None);
        // An embedded NUL would be truncated silently downstream.
        assert_eq!(file_url_to_path("file:///tmp/a%00b.sqlite"), None);
        // No path component at all.
        assert_eq!(file_url_to_path("file://localhost"), None);
    }

    #[test]
    fn query_and_fragment_are_not_part_of_the_path() {
        assert_eq!(
            file_url_to_path("file:///tmp/db.sqlite?x=1").as_deref(),
            Some("/tmp/db.sqlite")
        );
        assert_eq!(
            file_url_to_path("file:///tmp/db.sqlite#frag").as_deref(),
            Some("/tmp/db.sqlite")
        );
    }

    #[test]
    fn an_open_before_the_webview_is_ready_is_stashed_not_emitted() {
        // The cold-launch case. Emitting here would reach nobody and the launch
        // would show an empty window.
        let pending = PendingOpen::default();
        assert_eq!(pending.deliver("/tmp/db.sqlite".to_string()), None);
        assert_eq!(
            pending.take_initial(None).as_deref(),
            Some("/tmp/db.sqlite")
        );
    }

    #[test]
    fn an_open_after_the_webview_is_ready_is_emitted_not_stashed() {
        // The app-already-running case: the page is listening, so the path goes
        // out as an event. It must NOT also be stashed - a later
        // `get_initial_file` would then open the same file a second time.
        let pending = PendingOpen::default();
        pending.take_initial(None);
        assert_eq!(
            pending.deliver("/tmp/db.sqlite".to_string()),
            Some("/tmp/db.sqlite".to_string())
        );
        assert_eq!(pending.take_initial(None), None);
    }

    #[test]
    fn a_stashed_path_wins_over_the_launch_argument() {
        // Both can be present on a cold launch. The OS event names the document
        // the user actually double-clicked; argv on macOS is the bundle's own
        // arguments and can be stale or irrelevant.
        let pending = PendingOpen::default();
        pending.deliver("/tmp/opened.sqlite".to_string());
        assert_eq!(
            pending
                .take_initial(Some("/tmp/argv.sqlite".to_string()))
                .as_deref(),
            Some("/tmp/opened.sqlite")
        );
    }

    #[test]
    fn the_launch_argument_is_used_when_nothing_was_stashed() {
        // Windows and Linux only ever take this route.
        let pending = PendingOpen::default();
        assert_eq!(
            pending
                .take_initial(Some("/tmp/argv.sqlite".to_string()))
                .as_deref(),
            Some("/tmp/argv.sqlite")
        );
        assert_eq!(pending.take_initial(None), None);
    }

    #[test]
    fn a_relative_launch_argument_resolves_against_the_working_directory() {
        // The regression: a relative argv path reached `path_to_sqlite_uri`
        // untouched, which turned "inventory.sqlite" into
        // "file:/inventory.sqlite?immutable=1" - the FILESYSTEM ROOT, not the
        // shell's directory. So `dblitz inventory.sqlite` opened the wrong file
        // or none at all.
        let cwd = std::env::current_dir().expect("a working directory");
        let resolved = absolutize_launch_path("inventory.sqlite");

        assert!(
            std::path::Path::new(&resolved).is_absolute(),
            "a relative launch argument must be made absolute, got {resolved:?}"
        );
        assert_eq!(
            std::path::Path::new(&resolved),
            cwd.join("inventory.sqlite"),
            "it must resolve against the working directory, not the root"
        );
        // The defect's exact signature, stated directly so a future change that
        // reintroduces it fails here and not three layers away.
        assert_ne!(
            crate::db::path_to_sqlite_uri(&resolved),
            "file:/inventory.sqlite?immutable=1"
        );
    }

    #[test]
    fn every_caller_of_the_launch_argument_gets_the_absolute_path() {
        // Two callers read `argv[1]`: the webview, through `get_initial_file`,
        // and - on Windows - the duplicate-instance check in `run()`, which
        // hashes it to find a window already showing this file. `run()` used to
        // read it raw, so the two hashed different spellings of one path and
        // `dblitz inventory.db`, run from the directory holding it, opened a
        // second window on a file that was already open. They now share this
        // one resolver, which is what stops them drifting apart again.
        let cwd = std::env::current_dir().expect("a working directory");

        let resolved = resolve_launch_argument(&["inventory.sqlite".to_string()], |_| true)
            .expect("a present argument stays present");
        assert_eq!(
            std::path::Path::new(&resolved),
            cwd.join("inventory.sqlite")
        );

        assert_eq!(
            resolve_launch_argument(&[], |_| true),
            None,
            "no argument, no path"
        );
    }

    fn args(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_path_split_at_its_spaces_is_joined_back_together() {
        // The regression: after an in-app update, Tauri's NSIS installer
        // relaunched dblitz with the path unquoted, so a file under
        // "Shared Files - Archive" opened the directory "Shared" instead.
        let cwd = std::env::current_dir().expect("a working directory");
        let real = cwd.join("Shared Files - Archive").join("inventory.sqlite");
        let real = real.to_string_lossy().into_owned();
        let split: Vec<String> = real.split(' ').map(str::to_string).collect();
        assert_eq!(split.len(), 4, "the fixture must actually be split");

        let resolved = resolve_launch_argument(&split, |p| p == real);
        assert_eq!(resolved.as_deref(), Some(real.as_str()));
    }

    #[test]
    fn extra_arguments_are_not_joined_when_the_first_one_is_a_file() {
        // Control: a first argument that exists is the path, whatever follows.
        let cwd = std::env::current_dir().expect("a working directory");
        let first = cwd.join("a.sqlite").to_string_lossy().into_owned();
        let joined = format!("{first} extra");
        let resolved =
            resolve_launch_argument(&args(&[&first, "extra"]), |p| p == first || p == joined);
        assert_eq!(resolved.as_deref(), Some(first.as_str()));
    }

    #[test]
    fn a_missing_path_is_reported_as_given_when_joining_finds_nothing() {
        // Neither spelling exists: keep the first argument, so the open fails
        // with SQLite's error about the path the user can see, as before.
        let cwd = std::env::current_dir().expect("a working directory");
        let first = cwd.join("missing.sqlite").to_string_lossy().into_owned();
        let resolved = resolve_launch_argument(&args(&[&first, "more"]), |_| false);
        assert_eq!(resolved.as_deref(), Some(first.as_str()));
    }

    #[test]
    fn an_absolute_launch_argument_is_passed_through_unchanged() {
        // Control: the fix must not rewrite the path every other entry point
        // already hands over absolute.
        let cwd = std::env::current_dir().expect("a working directory");
        let absolute = cwd.join("db.sqlite");
        let absolute = absolute.to_string_lossy().into_owned();
        assert_eq!(absolutize_launch_path(&absolute), absolute);
    }
}
