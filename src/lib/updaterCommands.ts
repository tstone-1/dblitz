/**
 * Thin adapter over `tauri-plugin-updater` and `tauri-plugin-process`.
 *
 * This is the ONLY module that imports those two plugins, for the same reason
 * `ipc.ts` is the only module that calls `invoke`: it keeps
 * `updateState.svelte.ts` a state machine over a small typed surface, and keeps
 * it testable under vitest, where no Tauri runtime exists underneath.
 *
 * Unlike `ipc.ts` these are not passthroughs of our own commands — the updater
 * is a Tauri *plugin*, so this file is where its shape gets pinned down to what
 * the UI actually needs.
 */
import { check as checkForUpdate } from "@tauri-apps/plugin-updater";
import { relaunch as relaunchApp } from "@tauri-apps/plugin-process";
import { openUrl } from "@tauri-apps/plugin-opener";

/**
 * What happens while {@link PendingUpdate.download} is in flight.
 *
 * The two are reported separately because the plugin's `downloadAndInstall`
 * *both* downloads and installs before it resolves: by the time the promise
 * settles the install is already done. Its `Finished` event is the only moment
 * the caller can observe the handover, so it is surfaced as its own event kind
 * rather than as a 100% progress report — otherwise the UI can only ever show
 * "Installing…" after the fact.
 */
export type DownloadEvent =
  | { kind: "progress"; downloaded: number; total: number | null }
  | { kind: "installing" };

/**
 * What the UI needs from a pending update. The plugin's own `Update` object owns
 * a live download handle, so it is captured in the `download` closure rather
 * than reconstructed.
 *
 * Deliberately carries no release notes: dblitz publishes releases whose body is
 * a pointer to CHANGELOG.md, so rendering it would be strictly worse than the
 * link the update bar already shows.
 */
export type PendingUpdate = {
  version: string;
  download: (onEvent: (event: DownloadEvent) => void) => Promise<void>;
};

/**
 * Resolves to null when the running build is already current. Rejects when the
 * endpoint is unreachable or the manifest is unusable — callers decide whether
 * that is worth showing (a failed background check is not).
 */
export async function checkForUpdates(): Promise<PendingUpdate | null> {
  const update = await checkForUpdate();
  if (!update) return null;

  return {
    version: update.version,
    download: async (onEvent) => {
      let downloaded = 0;
      let total: number | null = null;
      // downloadAndInstall streams three event kinds. Only Started carries the
      // content length, and Progress reports per-chunk deltas rather than a
      // running total, so the accumulation has to happen here.
      //
      // Finished means the *download* is complete; the install then runs inside
      // this same call, before the promise resolves. That is the point at which
      // the UI should stop saying "Downloading", so it is reported as
      // `installing` rather than as a last progress tick.
      await update.downloadAndInstall((event) => {
        switch (event.event) {
          case "Started":
            total = event.data.contentLength ?? null;
            onEvent({ kind: "progress", downloaded: 0, total });
            break;
          case "Progress":
            downloaded += event.data.chunkLength;
            onEvent({ kind: "progress", downloaded, total });
            break;
          case "Finished":
            onEvent({ kind: "installing" });
            break;
        }
      });
    },
  };
}

/**
 * Restart into the freshly installed build. On Windows the NSIS installer has
 * already terminated the app by the time this would run, so in practice this is
 * the macOS/AppImage path — it must never be the thing that throws.
 */
export function relaunch(): Promise<void> {
  return relaunchApp();
}

/**
 * Opens a URL in the user's real browser. Not strictly part of the updater, but
 * the manual-download escape hatch is useless without it, and a plain
 * `<a target="_blank">` is not an option: the app's CSP is `default-src 'self'`,
 * so an in-webview navigation to github.com would be blocked rather than handed
 * to the browser. Covered by the existing `opener:default` capability.
 */
export function openExternal(url: string): Promise<void> {
  return openUrl(url);
}
