import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import {
  UpdateState,
  RELEASES_URL,
  STARTUP_CHECK_DELAY_MS,
  type UpdatePhase,
} from "./updateState.svelte";
import type { DownloadEvent, PendingUpdate } from "./updaterCommands";

// The adapter is the seam: it owns every @tauri-apps/plugin-* import, so mocking
// it here is what lets the state machine be tested without a Tauri runtime.
vi.mock("$lib/updaterCommands", () => ({
  checkForUpdates: vi.fn(),
  relaunch: vi.fn(),
  openExternal: vi.fn(),
}));

vi.mock("$lib/ipc", () => ({
  updateStatus: vi.fn(),
  getCheckForUpdatesOnStartup: vi.fn(),
  setCheckForUpdatesOnStartup: vi.fn(),
}));

const { checkForUpdates, relaunch, openExternal } = await import("$lib/updaterCommands");
const { updateStatus, getCheckForUpdatesOnStartup, setCheckForUpdatesOnStartup } =
  await import("$lib/ipc");

const checkMock = vi.mocked(checkForUpdates);
const relaunchMock = vi.mocked(relaunch);
const openExternalMock = vi.mocked(openExternal);
const statusMock = vi.mocked(updateStatus);
const getPrefMock = vi.mocked(getCheckForUpdatesOnStartup);
const setPrefMock = vi.mocked(setCheckForUpdatesOnStartup);

function pending(version = "26.8.0", download?: PendingUpdate["download"]): PendingUpdate {
  return {
    version,
    download: download ?? vi.fn().mockResolvedValue(undefined),
  };
}

beforeEach(() => {
  vi.clearAllMocks();
  // The state machine logs through console on purpose (dblitz has no frontend
  // log plugin); silence it so a passing run stays quiet.
  vi.spyOn(console, "warn").mockImplementation(() => {});
  vi.spyOn(console, "error").mockImplementation(() => {});
});

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

describe("UpdateState.check", () => {
  it("reports up to date when no update is offered", async () => {
    checkMock.mockResolvedValue(null);
    const state = new UpdateState();

    await state.check("manual");

    expect(state.phase).toEqual({ kind: "upToDate" });
    expect(state.showBar).toBe(false);
  });

  it("surfaces an available update and shows the bar", async () => {
    checkMock.mockResolvedValue(pending("26.8.0"));
    const state = new UpdateState();

    await state.check("manual");

    expect(state.phase).toEqual({ kind: "available", version: "26.8.0" });
    expect(state.showBar).toBe(true);
  });

  it("stays silent when a background check fails", async () => {
    // An unreachable endpoint at launch is not something the user asked about
    // or can act on — surfacing it would be pure noise.
    checkMock.mockRejectedValue(new Error("offline"));
    const state = new UpdateState();

    await state.check("startup");

    expect(state.phase).toEqual({ kind: "idle" });
    expect(state.showBar).toBe(false);
    // Still logged, just not shown.
    expect(console.warn).toHaveBeenCalled();
    expect(console.error).not.toHaveBeenCalled();
  });

  it("reports an error when a check the user asked for fails", async () => {
    checkMock.mockRejectedValue(new Error("offline"));
    const state = new UpdateState();

    await state.check("manual");

    expect(state.phase.kind).toBe("error");
    expect(state.showBar).toBe(true);
  });

  it("ignores a second check while one is in flight", async () => {
    let release: (value: null) => void = () => {};
    checkMock.mockReturnValue(
      new Promise<null>((resolve) => {
        release = resolve;
      }),
    );
    const state = new UpdateState();

    const first = state.check("manual");
    await state.check("manual");
    expect(checkMock).toHaveBeenCalledTimes(1);

    release(null);
    await first;
  });

  it("clears a previous dismissal when a new update appears", async () => {
    checkMock.mockResolvedValue(pending("26.8.0"));
    const state = new UpdateState();
    state.dismiss();

    await state.check("manual");

    expect(state.dismissed).toBe(false);
    expect(state.showBar).toBe(true);
  });
});

describe("UpdateState.installAndRestart", () => {
  it("reports download progress and relaunches", async () => {
    const state = new UpdateState();
    const seen: UpdatePhase[] = [];
    const download = vi.fn(async (onEvent: (event: DownloadEvent) => void) => {
      onEvent({ kind: "progress", downloaded: 0, total: 100 });
      seen.push(state.phase);
      onEvent({ kind: "progress", downloaded: 40, total: 100 });
      seen.push(state.phase);
    });
    checkMock.mockResolvedValue(pending("26.8.0", download));
    await state.check("manual");

    await state.installAndRestart();

    expect(seen).toEqual([
      { kind: "downloading", version: "26.8.0", downloaded: 0, total: 100 },
      { kind: "downloading", version: "26.8.0", downloaded: 40, total: 100 },
    ]);
    expect(relaunchMock).toHaveBeenCalledTimes(1);
    expect(state.phase).toEqual({ kind: "installing", version: "26.8.0" });
  });

  it("shows the install phase while the install is still running", async () => {
    // The whole point of the separate event kind: downloadAndInstall installs
    // before it resolves, so a phase flipped after the promise settles would be
    // shown once the install is already over.
    const state = new UpdateState();
    const seen: UpdatePhase[] = [];
    let relaunchedDuringInstall = false;
    const download = vi.fn(async (onEvent: (event: DownloadEvent) => void) => {
      onEvent({ kind: "progress", downloaded: 100, total: 100 });
      onEvent({ kind: "installing" });
      // Observed from inside the still-unresolved call: this is the window the
      // user actually spends waiting for the install.
      seen.push(state.phase);
      relaunchedDuringInstall = relaunchMock.mock.calls.length > 0;
      await Promise.resolve();
    });
    checkMock.mockResolvedValue(pending("26.8.0", download));
    await state.check("manual");

    await state.installAndRestart();

    expect(seen).toEqual([{ kind: "installing", version: "26.8.0" }]);
    // Relaunch belongs strictly after the call resolves — restarting mid-install
    // would abort it.
    expect(relaunchedDuringInstall).toBe(false);
    expect(relaunchMock).toHaveBeenCalledTimes(1);
  });

  it("still reaches the install phase when the handover is never reported", async () => {
    // Fallback path: whatever the plugin does or doesn't emit, the bar must not
    // claim a download is in progress while the app restarts underneath it.
    const download = vi.fn(async (onEvent: (event: DownloadEvent) => void) => {
      onEvent({ kind: "progress", downloaded: 10, total: null });
    });
    checkMock.mockResolvedValue(pending("26.8.0", download));
    const state = new UpdateState();
    await state.check("manual");

    await state.installAndRestart();

    expect(state.phase).toEqual({ kind: "installing", version: "26.8.0" });
    expect(relaunchMock).toHaveBeenCalledTimes(1);
  });

  it("offers a manual download when the install fails", async () => {
    // The realistic failure: the app is somewhere unwritable, which no amount of
    // retrying the same download can fix.
    const download = vi.fn().mockRejectedValue(new Error("Permission denied"));
    checkMock.mockResolvedValue(pending("26.8.0", download));
    const state = new UpdateState();
    await state.check("manual");

    await state.installAndRestart();

    expect(state.phase.kind).toBe("error");
    expect(state.phase.kind === "error" && state.phase.message).toContain("26.8.0");
    expect(relaunchMock).not.toHaveBeenCalled();
  });

  it("does nothing without a pending update", async () => {
    const state = new UpdateState();

    await state.installAndRestart();

    expect(relaunchMock).not.toHaveBeenCalled();
    expect(state.phase).toEqual({ kind: "idle" });
  });

  it("refuses to install on an install that can't replace itself", async () => {
    // Linux .deb/.rpm: the plugin would fail deep inside with an opaque error,
    // so the refusal happens here where the reason can be explained.
    const download = vi.fn().mockResolvedValue(undefined);
    checkMock.mockResolvedValue(pending("26.8.0", download));
    const state = new UpdateState();
    state.selfUpdateSupported = false;
    await state.check("manual");

    await state.installAndRestart();

    expect(download).not.toHaveBeenCalled();
    expect(relaunchMock).not.toHaveBeenCalled();
    expect(state.phase.kind).toBe("error");
    expect(state.phase.kind === "error" && state.phase.message).toContain("GitHub");
  });
});

describe("UpdateState.scheduleStartupCheck", () => {
  it("evaluates the opt-out when the timer fires, not when it is scheduled", async () => {
    // The preference loads asynchronously over IPC, so at schedule time it is
    // still at its default. Reading it eagerly would check for updates for a
    // user who had turned that off — the bug this ordering exists to prevent.
    vi.useFakeTimers();
    checkMock.mockResolvedValue(null);
    const state = new UpdateState();

    state.scheduleStartupCheck();
    state.checkAtStartup = false;
    await vi.advanceTimersByTimeAsync(STARTUP_CHECK_DELAY_MS);

    expect(checkMock).not.toHaveBeenCalled();
  });

  it("checks once the delay elapses when enabled", async () => {
    vi.useFakeTimers();
    checkMock.mockResolvedValue(null);
    const state = new UpdateState();

    state.scheduleStartupCheck();
    await vi.advanceTimersByTimeAsync(STARTUP_CHECK_DELAY_MS);

    expect(checkMock).toHaveBeenCalledTimes(1);
  });

  it("does not check before the delay elapses", async () => {
    vi.useFakeTimers();
    checkMock.mockResolvedValue(null);
    const state = new UpdateState();

    state.scheduleStartupCheck();
    await vi.advanceTimersByTimeAsync(STARTUP_CHECK_DELAY_MS - 1);

    expect(checkMock).not.toHaveBeenCalled();
  });

  it("cancels a pending check on teardown", async () => {
    vi.useFakeTimers();
    checkMock.mockResolvedValue(null);
    const state = new UpdateState();

    const teardown = state.scheduleStartupCheck();
    teardown();
    await vi.advanceTimersByTimeAsync(STARTUP_CHECK_DELAY_MS);

    expect(checkMock).not.toHaveBeenCalled();
  });
});

describe("UpdateState.loadStatus", () => {
  it("adopts the backend's flags verbatim", async () => {
    statusMock.mockResolvedValue({
      previousVersion: "26.7.5",
      currentVersion: "26.8.0",
      updated: true,
      selfUpdateSupported: true,
    });
    const state = new UpdateState();

    await state.loadStatus();

    expect(state.currentVersion).toBe("26.8.0");
    expect(state.justUpdated).toBe(true);
    expect(state.showUpdatedNotice).toBe(true);
  });

  it("carries the self-update capability through to the install gate", async () => {
    statusMock.mockResolvedValue({
      previousVersion: null,
      currentVersion: "26.8.0",
      updated: false,
      selfUpdateSupported: false,
    });
    const state = new UpdateState();

    await state.loadStatus();

    expect(state.canInstall).toBe(false);
    expect(state.showUpdatedNotice).toBe(false);
  });

  it("leaves the version blank but stays optimistic when the call fails", async () => {
    statusMock.mockRejectedValue(new Error("no runtime"));
    const state = new UpdateState();

    await state.loadStatus();

    expect(state.currentVersion).toBeNull();
    expect(state.justUpdated).toBe(false);
    // A failed status read must not disable a working updater on macOS/Windows.
    expect(state.canInstall).toBe(true);
  });

  it("lets the post-update notice be dismissed", async () => {
    statusMock.mockResolvedValue({
      previousVersion: "26.7.5",
      currentVersion: "26.8.0",
      updated: true,
      selfUpdateSupported: true,
    });
    const state = new UpdateState();
    await state.loadStatus();

    state.dismissUpdatedNotice();

    expect(state.showUpdatedNotice).toBe(false);
  });
});

describe("UpdateState startup preference", () => {
  it("adopts a stored opt-out", async () => {
    getPrefMock.mockResolvedValue(false);
    const state = new UpdateState();

    await state.loadStartupPreference();

    expect(state.checkAtStartup).toBe(false);
  });

  it("keeps the default when the preference can't be read", async () => {
    getPrefMock.mockRejectedValue(new Error("no runtime"));
    const state = new UpdateState();

    await state.loadStartupPreference();

    expect(state.checkAtStartup).toBe(true);
  });

  it("persists a change", async () => {
    setPrefMock.mockResolvedValue(undefined);
    const state = new UpdateState();

    await state.setCheckAtStartup(false);

    expect(setPrefMock).toHaveBeenCalledWith(false);
    expect(state.checkAtStartup).toBe(false);
  });

  it("snaps back when the change can't be persisted", async () => {
    // Otherwise the toggle claims a preference that won't survive a restart.
    setPrefMock.mockRejectedValue(new Error("disk full"));
    const state = new UpdateState();

    await state.setCheckAtStartup(false);

    expect(state.checkAtStartup).toBe(true);
  });
});

describe("UpdateState.openReleasesPage", () => {
  it("opens the releases page in the real browser", async () => {
    openExternalMock.mockResolvedValue(undefined);
    const state = new UpdateState();

    await state.openReleasesPage();

    expect(openExternalMock).toHaveBeenCalledWith(RELEASES_URL);
  });

  it("swallows a failure to open the browser", async () => {
    // The escape hatch failing is not worth replacing the error the user is
    // already looking at.
    openExternalMock.mockRejectedValue(new Error("no handler"));
    const state = new UpdateState();

    await expect(state.openReleasesPage()).resolves.toBeUndefined();
  });
});

describe("UpdateState.reset", () => {
  it("clears a terminal phase", async () => {
    checkMock.mockResolvedValue(null);
    const state = new UpdateState();
    await state.check("manual");

    state.reset();

    expect(state.phase).toEqual({ kind: "idle" });
  });

  it("leaves a genuinely pending update alone", async () => {
    checkMock.mockResolvedValue(pending("26.8.0"));
    const state = new UpdateState();
    await state.check("manual");

    state.reset();

    expect(state.phase).toEqual({ kind: "available", version: "26.8.0" });
  });
});
