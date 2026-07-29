<script lang="ts">
  import { appState } from "$lib/store.svelte";
  import { update } from "$lib/updateState.svelte";

  // App-global status bars: the error bar, the notice bar and the two update
  // bars. They are not toolbar content -- they belong to the window, not to the
  // Open/tabs/Settings row -- so +page.svelte mounts this directly below
  // <Toolbar /> and owns their placement explicitly.
</script>

{#if appState.error}
  <div class="error-bar">
    {appState.error}
    <button onclick={() => (appState.error = null)}>dismiss</button>
  </div>
{/if}

{#if appState.notice}
  <div class="notice-bar">
    {appState.notice}
    <button onclick={() => (appState.notice = null)}>dismiss</button>
  </div>
{/if}

<!-- One-time confirmation that a self-update actually landed. Without it the
     update is completely invisible: the app downloads, restarts, and looks
     identical apart from the version in the title bar. -->
{#if update.showUpdatedNotice}
  <div class="update-bar">
    <span>dblitz was updated to v{update.currentVersion}.</span>
    <button onclick={() => void update.openReleasesPage()}>What's new</button>
    <button onclick={() => update.dismissUpdatedNotice()}>dismiss</button>
  </div>
{/if}

{#if update.showBar}
  <div class="update-bar" role="status">
    {#if update.phase.kind === "available"}
      <span>Version {update.phase.version} is available.</span>
      {#if update.canInstall}
        <button class="update-install" onclick={() => void update.installAndRestart()}>
          Install and restart
        </button>
      {:else}
        <!-- A .deb/.rpm install: the Tauri updater can only replace an AppImage,
             so offering an Install button here would promise something that
             cannot work. Tell the user why and hand them the download. -->
        <span class="update-detail">This installation can't update itself.</span>
      {/if}
      <button onclick={() => void update.openReleasesPage()}>Open GitHub</button>
      <button onclick={() => update.dismiss()}>dismiss</button>
    {:else if update.phase.kind === "downloading"}
      <span>Downloading {update.phase.version}…</span>
      <!-- `total` is null until the Started event lands, and some servers omit
           the content length entirely — fall back to an indeterminate bar rather
           than showing a bogus 0%. -->
      <progress
        class="update-progress"
        max={update.phase.total ?? undefined}
        value={update.phase.total === null ? undefined : update.phase.downloaded}
      ></progress>
    {:else if update.phase.kind === "installing"}
      <span>Installing {update.phase.version}… dblitz will restart.</span>
    {:else if update.phase.kind === "error"}
      <span>{update.phase.message}</span>
      <button onclick={() => void update.openReleasesPage()}>Open GitHub</button>
      <button onclick={() => update.dismiss()}>dismiss</button>
    {/if}
  </div>
{/if}

<style>
  .error-bar {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 4px 12px;
    background: var(--error);
    color: white;
    font-size: 12px;
    flex-shrink: 0;
  }
  .error-bar button {
    background: transparent;
    border: 1px solid white;
    color: white;
    padding: 1px 8px;
    font-size: 11px;
  }

  .notice-bar {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 4px 12px;
    background: color-mix(in srgb, var(--warning) 18%, var(--bg-secondary));
    color: var(--text-primary);
    border-bottom: 1px solid var(--warning);
    font-size: 12px;
    flex-shrink: 0;
  }
  .notice-bar button {
    background: transparent;
    border: 1px solid var(--text-muted);
    color: var(--text-primary);
    padding: 1px 8px;
    font-size: 11px;
  }

  /* Same bar family as .error-bar/.notice-bar, but informational: an available
     update is good news, so it borrows the accent rather than the warning color. */
  .update-bar {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 4px 12px;
    background: color-mix(in srgb, var(--accent) 14%, var(--bg-secondary));
    color: var(--text-primary);
    border-bottom: 1px solid var(--accent);
    font-size: 12px;
    flex-shrink: 0;
  }
  .update-bar button {
    background: transparent;
    border: 1px solid var(--text-muted);
    color: var(--text-primary);
    padding: 1px 8px;
    font-size: 11px;
  }
  .update-bar button.update-install {
    background: var(--accent);
    border-color: var(--accent);
    color: white;
    font-weight: 600;
  }
  .update-bar button.update-install:hover {
    background: var(--accent-hover);
    border-color: var(--accent-hover);
  }

  .update-detail {
    color: var(--text-secondary);
    font-size: 11px;
  }

  .update-progress {
    width: 160px;
    height: 6px;
  }
</style>
