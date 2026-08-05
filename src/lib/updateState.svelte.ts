import {
  updateStatus,
  getCheckForUpdatesOnStartup,
  setCheckForUpdatesOnStartup,
} from "$lib/ipc";
import {
  checkForUpdates,
  relaunch,
  openExternal,
  type PendingUpdate,
} from "$lib/updaterCommands";

/**
 * Where the release lives when the in-app path can't be used. Shown on every
 * error state, and as the only action on a `.deb`/`.rpm` Linux install: a user
 * whose install location isn't writable, or isn't an AppImage, can never be
 * rescued by retrying the same download.
 */
export const RELEASES_URL = "https://github.com/tstone-1/dblitz/releases/latest";

/**
 * A background check that fails is not worth surfacing — the user didn't ask,
 * and GitHub being briefly unreachable is not their problem. A check they
 * explicitly clicked is the opposite: silence would read as a broken button.
 */
export type CheckOrigin = "startup" | "manual";

export type UpdatePhase =
  | { kind: "idle" }
  | { kind: "checking" }
  | { kind: "upToDate" }
  | { kind: "available"; version: string }
  | { kind: "downloading"; version: string; downloaded: number; total: number | null }
  | { kind: "installing"; version: string }
  | { kind: "error"; message: string };

/**
 * Delay before the automatic startup check. Long enough that it never contends
 * with opening the database the app was launched for (dblitz is usually started
 * *by* double-clicking a `.db`), short enough that the user is still in the app
 * when the bar appears.
 */
export const STARTUP_CHECK_DELAY_MS = 10_000;

export class UpdateState {
  phase = $state<UpdatePhase>({ kind: "idle" });
  /**
   * Dismissal is per-sighting, not persisted: an update the user waved away
   * should come back on the next launch, or it will never be installed.
   */
  dismissed = $state(false);

  /** The running build's version — null until {@link loadStatus} lands. */
  currentVersion = $state<string | null>(null);
  /**
   * Whether this launch is the first after an update, used for the one-time
   * confirmation notice. Comes from Rust so the "a first run is not an update"
   * rule lives in exactly one place (`src-tauri/src/updates.rs`).
   */
  justUpdated = $state(false);
  /**
   * Whether this install can replace itself. Optimistic default: on the two
   * platforms where it is always true, a failed status call should not disable a
   * working updater, and the false case (Linux `.deb`/`.rpm`) degrades to the
   * same manual-download link an install failure would show anyway.
   */
  selfUpdateSupported = $state(true);
  /** Separately dismissible from an update offer — they can be on screen together. */
  updatedNoticeDismissed = $state(false);

  /**
   * Persisted opt-out for the automatic check. Defaults to true and is
   * overwritten by {@link loadStartupPreference}; see
   * {@link scheduleStartupCheck} for why the default matters.
   */
  checkAtStartup = $state(true);

  #pending: PendingUpdate | null = null;
  #startupTimer: ReturnType<typeof setTimeout> | null = null;

  showBar = $derived(
    !this.dismissed &&
      (this.phase.kind === "available" ||
        this.phase.kind === "downloading" ||
        this.phase.kind === "installing" ||
        this.phase.kind === "error"),
  );

  /** True on the first launch after an update, until dismissed. */
  showUpdatedNotice = $derived(this.justUpdated && !this.updatedNoticeDismissed);

  /**
   * True while a check or install is in flight, so the manual-check button can
   * disable itself rather than stacking concurrent checks.
   */
  busy = $derived(
    this.phase.kind === "checking" ||
      this.phase.kind === "downloading" ||
      this.phase.kind === "installing",
  );

  /** Whether the update bar may offer an Install button at all. */
  canInstall = $derived(this.selfUpdateSupported);

  /**
   * Best-effort: the version row in Settings simply stays blank if this fails,
   * which is not worth surfacing as an error.
   */
  async loadStatus(): Promise<void> {
    try {
      const status = await updateStatus();
      this.currentVersion = status.currentVersion;
      this.justUpdated = status.updated;
      this.selfUpdateSupported = status.selfUpdateSupported;
    } catch (e) {
      console.warn("Could not read the update status:", e);
    }
  }

  /**
   * Best-effort: on failure the flag keeps its default, so a user who opted out
   * gets one unwanted check rather than an app that silently stops updating.
   */
  async loadStartupPreference(): Promise<void> {
    try {
      this.checkAtStartup = await getCheckForUpdatesOnStartup();
    } catch (e) {
      console.warn("Could not read the update-check preference:", e);
    }
  }

  /** Persists the opt-out. Applies optimistically so the toggle feels instant. */
  async setCheckAtStartup(enabled: boolean): Promise<void> {
    this.checkAtStartup = enabled;
    try {
      await setCheckForUpdatesOnStartup(enabled);
    } catch (e) {
      console.error("Could not save the update-check preference:", e);
      // Snap back rather than leave the UI claiming a preference that didn't
      // persist and won't survive a restart.
      this.checkAtStartup = !enabled;
    }
  }

  /**
   * Schedules the automatic startup check. Returns a teardown that cancels a
   * check not yet fired, so a page teardown can't leave a timer running against
   * a torn-down state object.
   *
   * `checkAtStartup` is read when the timer fires, not when it is scheduled:
   * the preference loads asynchronously over IPC, so at schedule time it is
   * still sitting at its default `true`, and a snapshot taken here would check
   * for updates for users who had turned that off.
   */
  scheduleStartupCheck(): () => void {
    this.#startupTimer = setTimeout(() => {
      this.#startupTimer = null;
      if (!this.checkAtStartup) return;
      void this.check("startup");
    }, STARTUP_CHECK_DELAY_MS);
    return () => {
      if (this.#startupTimer !== null) {
        clearTimeout(this.#startupTimer);
        this.#startupTimer = null;
      }
    };
  }

  async check(origin: CheckOrigin): Promise<void> {
    // A second check while one is running would race two downloads onto the
    // same install path for no benefit.
    if (this.busy) return;

    this.phase = { kind: "checking" };
    try {
      const update = await checkForUpdates();
      this.#pending = update;
      if (!update) {
        this.phase = { kind: "upToDate" };
        return;
      }
      this.dismissed = false;
      this.phase = { kind: "available", version: update.version };
    } catch (e) {
      this.#pending = null;
      if (origin === "startup") {
        // Silent by design: an unreachable endpoint at launch is noise, not a
        // problem the user can act on.
        console.warn("Background update check failed:", e);
        this.phase = { kind: "idle" };
        return;
      }
      console.error("Update check failed:", e);
      this.phase = {
        kind: "error",
        message: "Couldn't check for updates. Check your connection and try again.",
      };
    }
  }

  /**
   * Downloads and installs the pending update, then restarts. Windows never
   * reaches the relaunch — the NSIS installer terminates the app first.
   *
   * The download call installs too, and only resolves once that is done, so the
   * `installing` phase has to come from the adapter's own event stream — waiting
   * for the promise would show "Installing…" for the instant between a finished
   * install and the relaunch, while the install itself sat behind a progress bar
   * frozen at 100%.
   */
  async installAndRestart(): Promise<void> {
    const update = this.#pending;
    if (!update || this.busy) return;
    // Defensive: the UI hides the button on a .deb/.rpm install, but an install
    // attempt there fails deep inside the plugin with an opaque message, so
    // refuse it here where the reason can actually be explained.
    if (!this.selfUpdateSupported) {
      this.phase = {
        kind: "error",
        message: `This installation can't update itself. Download ${update.version} from GitHub instead.`,
      };
      return;
    }

    this.phase = { kind: "downloading", version: update.version, downloaded: 0, total: null };
    try {
      let installing = false;
      await update.download((event) => {
        if (event.kind === "installing") {
          installing = true;
          this.phase = { kind: "installing", version: update.version };
          return;
        }
        this.phase = {
          kind: "downloading",
          version: update.version,
          downloaded: event.downloaded,
          total: event.total,
        };
      });
      // Fallback, not the normal path: a download that resolved without ever
      // reporting the handover must not leave the bar claiming a download is
      // still running while the app restarts underneath it.
      if (!installing) {
        this.phase = { kind: "installing", version: update.version };
      }
      await relaunch();
    } catch (e) {
      console.error("Update install failed:", e);
      this.phase = {
        kind: "error",
        message: `Couldn't install ${update.version}. Download it manually instead.`,
      };
    }
  }

  /**
   * The escape hatch behind every error state and the only action offered on an
   * install that can't replace itself. Goes through the state object rather than
   * the component so the UI imports one module, not two.
   */
  async openReleasesPage(): Promise<void> {
    try {
      await openExternal(RELEASES_URL);
    } catch (e) {
      console.error("Could not open the releases page:", e);
    }
  }

  dismiss(): void {
    this.dismissed = true;
  }

  dismissUpdatedNotice(): void {
    this.updatedNoticeDismissed = true;
  }

  /**
   * Clears a terminal message without dismissing a genuinely pending update, so
   * the Settings dropdown's "up to date"/error line doesn't linger forever.
   */
  reset(): void {
    if (this.phase.kind === "upToDate" || this.phase.kind === "error") {
      this.phase = { kind: "idle" };
    }
  }
}

export const update = new UpdateState();
