<script lang="ts">
  import { open } from "@tauri-apps/plugin-dialog";
  import { invoke } from "@tauri-apps/api/core";
  import { appState, openDatabase, closeDatabase, setTheme, type Theme } from "$lib/store.svelte";

  let showSettings = $state(false);
  let showRecents = $state(false);
  let recentFiles = $state<string[]>([]);

  async function handleOpen() {
    const path = await open({
      filters: [{ name: "SQLite", extensions: ["db", "sqlite", "sqlite3", "db3"] }],
      multiple: false,
      directory: false,
    });
    if (path) {
      await openDatabase(path as string);
    }
  }

  async function toggleRecents() {
    if (showRecents) {
      showRecents = false;
      return;
    }
    // Open the dropdown immediately so the chevron click feels responsive
    // even if the backend round-trip is slow (e.g. app.json on a sleeping
    // disk). The dropdown shows the previous list (or "No recent databases"
    // on the very first click) and then refreshes silently. Backend filters
    // out files that no longer exist, so the list stays self-cleaning.
    showRecents = true;
    try {
      recentFiles = await invoke<string[]>("get_recent_files");
    } catch (e) {
      console.error("Failed to load recent files:", e);
      recentFiles = [];
    }
  }

  async function openRecent(path: string) {
    showRecents = false;
    await openDatabase(path);
  }

  async function clearRecents() {
    try {
      await invoke("clear_recent_files");
      recentFiles = [];
    } catch (e) {
      console.error("Failed to clear recent files:", e);
    }
    showRecents = false;
  }

  function fileName(path: string | null): string {
    if (!path) return "No file open";
    const parts = path.replace(/\\/g, "/").split("/");
    return parts[parts.length - 1];
  }

  function parentDir(path: string): string {
    const norm = path.replace(/\\/g, "/");
    const idx = norm.lastIndexOf("/");
    return idx > 0 ? norm.slice(0, idx) : "";
  }

  function handleClickOutside(e: MouseEvent) {
    const target = e.target as HTMLElement;
    if (!target.closest(".settings-dropdown") && !target.closest(".settings-toggle")) {
      showSettings = false;
    }
    if (!target.closest(".recents-dropdown") && !target.closest(".open-chevron")) {
      showRecents = false;
    }
  }
</script>

<svelte:document onclick={handleClickOutside} />

<div class="toolbar">
  <div class="open-btn-group">
    <button onclick={handleOpen} class="open-btn" title="Open SQLite database">
      Open DB
    </button>
    <button
      onclick={toggleRecents}
      class="open-chevron"
      class:active={showRecents}
      title="Recent databases"
      aria-label="Recent databases"
      aria-expanded={showRecents}
      aria-haspopup="menu"
    >
      <svg viewBox="0 0 10 6" width="10" height="6" aria-hidden="true">
        <path d="M0 0 L5 6 L10 0 Z" fill="currentColor"/>
      </svg>
    </button>
    {#if showRecents}
      <div class="recents-dropdown" role="menu">
        {#if recentFiles.length === 0}
          <div class="recents-empty">No recent databases</div>
        {:else}
          {#each recentFiles as path}
            <button class="recent-item" role="menuitem" onclick={() => openRecent(path)} title={path}>
              <span class="recent-name">{fileName(path)}</span>
              <span class="recent-dir">{parentDir(path)}</span>
            </button>
          {/each}
          <div class="recents-sep"></div>
          <button class="recent-clear" role="menuitem" onclick={clearRecents}>Clear recent files</button>
        {/if}
      </div>
    {/if}
  </div>

  <span class="file-path" title={appState.dbPath ?? ""}>
    {appState.dbPath ?? ""}
  </span>

  {#if appState.dbPath}
    <span class="table-count">{appState.tables.length} table{appState.tables.length !== 1 ? 's' : ''}</span>
  {/if}

  <div class="tabs">
    <button class="tab" class:active={appState.activeTab === "structure"} onclick={() => (appState.activeTab = "structure")}>Structure</button>
    <button class="tab" class:active={appState.activeTab === "browse"} onclick={() => (appState.activeTab = "browse")}>Browse Data</button>
    <button class="tab" class:active={appState.activeTab === "sql"} onclick={() => (appState.activeTab = "sql")}>Execute SQL</button>
  </div>

  {#if appState.dbPath}
    <button onclick={() => closeDatabase()} class="unload-btn" title="Close database">
      Unload
    </button>
  {/if}

  {#if appState.loading}
    <span class="loading">Loading...</span>
  {/if}

  <div class="settings-wrapper">
    <button class="settings-toggle" onclick={() => (showSettings = !showSettings)} title="Settings">
      Settings
    </button>
    {#if showSettings}
      <div class="settings-dropdown">
        <div class="settings-section">
          <div class="settings-label">Theme</div>
          <div class="theme-options">
            <button class="theme-btn" class:active={appState.theme === 'light'} onclick={() => setTheme('light')}>Light</button>
            <button class="theme-btn" class:active={appState.theme === 'dark'} onclick={() => setTheme('dark')}>Dark</button>
          </div>
        </div>
      </div>
    {/if}
  </div>
</div>

{#if appState.error}
  <div class="error-bar">
    {appState.error}
    <button onclick={() => (appState.error = null)}>dismiss</button>
  </div>
{/if}

<style>
  .toolbar {
    display: flex;
    align-items: end;
    gap: 12px;
    padding: 6px 12px 6px;
    background: var(--bg-secondary);
    border-bottom: 1px solid var(--border-color);
    flex-shrink: 0;
  }

  .open-btn-group {
    position: relative;
    display: inline-flex;
    align-items: stretch;
  }

  .open-btn {
    background: var(--accent);
    color: white;
    font-weight: 600;
    border: none;
    padding: 5px 14px;
    border-radius: 4px 0 0 4px;
  }
  .open-btn:hover { background: var(--accent-hover); }

  .open-chevron {
    background: var(--accent);
    color: white;
    border: none;
    border-left: 1px solid color-mix(in srgb, white 25%, transparent);
    padding: 0 6px;
    border-radius: 0 4px 4px 0;
    display: flex;
    align-items: center;
    justify-content: center;
    cursor: pointer;
  }
  .open-chevron:hover,
  .open-chevron.active { background: var(--accent-hover); }

  .recents-dropdown {
    position: absolute;
    top: calc(100% + 4px);
    left: 0;
    min-width: 280px;
    max-width: 480px;
    background: var(--bg-secondary);
    border: 1px solid var(--border-color);
    border-radius: 6px;
    padding: 4px;
    z-index: 100;
    box-shadow: 0 4px 12px rgba(0,0,0,0.18);
    display: flex;
    flex-direction: column;
  }

  .recents-empty {
    padding: 10px 12px;
    color: var(--text-muted);
    font-size: 12px;
    font-style: italic;
    text-align: center;
  }

  .recent-item {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 1px;
    padding: 6px 10px;
    background: transparent;
    border: none;
    border-radius: 4px;
    text-align: left;
    cursor: pointer;
    color: var(--text-primary);
    overflow: hidden;
  }
  .recent-item:hover { background: var(--bg-hover); }

  .recent-name {
    font-size: 12px;
    font-weight: 500;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    max-width: 100%;
  }
  .recent-dir {
    font-size: 10px;
    color: var(--text-muted);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    max-width: 100%;
  }

  .recents-sep {
    height: 1px;
    background: var(--border-color);
    margin: 4px 2px;
  }

  .recent-clear {
    padding: 6px 10px;
    background: transparent;
    border: none;
    border-radius: 4px;
    text-align: left;
    cursor: pointer;
    color: var(--text-muted);
    font-size: 11px;
  }
  .recent-clear:hover { background: var(--bg-hover); color: var(--error); }

  .file-path {
    color: var(--text-secondary);
    font-size: 12px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    flex: 1;
    min-width: 0;
  }

  .table-count { color: var(--text-muted); font-size: 11px; }

  .tabs { display: flex; gap: 2px; margin-left: auto; margin-bottom: -7px; }

  .tab {
    padding: 5px 16px;
    border: 1px solid transparent;
    border-bottom: 1px solid transparent;
    border-radius: 4px 4px 0 0;
    background: transparent;
    color: var(--text-secondary);
    font-weight: 500;
  }
  .tab:hover { background: var(--bg-hover); color: var(--text-primary); }
  .tab.active { background: var(--bg-primary); color: var(--tab-active); border-color: var(--border-color); border-bottom-color: var(--bg-primary); }

  .loading { color: var(--warning); font-size: 11px; animation: pulse 1s infinite; }
  @keyframes pulse { 50% { opacity: 0.5; } }

  .unload-btn {
    font-size: 12px;
    padding: 4px 10px;
    color: var(--error);
    border-color: var(--error);
  }
  .unload-btn:hover {
    background: var(--error);
    color: white;
  }

  .settings-wrapper {
    position: relative;
  }

  .settings-toggle {
    font-size: 12px;
    padding: 4px 10px;
  }

  .settings-dropdown {
    position: absolute;
    top: 100%;
    right: 0;
    margin-top: 4px;
    background: var(--bg-secondary);
    border: 1px solid var(--border-color);
    border-radius: 6px;
    padding: 8px 12px;
    min-width: 160px;
    z-index: 100;
    box-shadow: 0 4px 12px rgba(0,0,0,0.15);
  }

  .settings-label {
    font-size: 11px;
    font-weight: 600;
    color: var(--text-muted);
    text-transform: uppercase;
    letter-spacing: 0.5px;
    margin-bottom: 6px;
  }

  .theme-options {
    display: flex;
    gap: 4px;
  }

  .theme-btn {
    flex: 1;
    padding: 4px 12px;
    font-size: 12px;
    border-radius: 4px;
    text-align: center;
  }

  .theme-btn.active {
    background: var(--accent);
    color: white;
    border-color: var(--accent);
  }

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
</style>
