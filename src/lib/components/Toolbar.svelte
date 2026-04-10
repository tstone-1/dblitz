<script lang="ts">
  import { open } from "@tauri-apps/plugin-dialog";
  import { appState, openDatabase, closeDatabase, setTheme, type Theme } from "$lib/store.svelte";

  let showSettings = $state(false);

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

  function fileName(path: string | null): string {
    if (!path) return "No file open";
    const parts = path.replace(/\\/g, "/").split("/");
    return parts[parts.length - 1];
  }

  function handleClickOutside(e: MouseEvent) {
    const target = e.target as HTMLElement;
    if (!target.closest(".settings-dropdown") && !target.closest(".settings-toggle")) {
      showSettings = false;
    }
  }
</script>

<svelte:document onclick={handleClickOutside} />

<div class="toolbar">
  <button onclick={handleOpen} class="open-btn" title="Open SQLite database">
    Open DB
  </button>

  <span class="file-path" title={appState.dbPath ?? ""}>
    {fileName(appState.dbPath)}
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

  .open-btn {
    background: var(--accent);
    color: white;
    font-weight: 600;
    border: none;
    padding: 5px 14px;
    border-radius: 4px;
  }
  .open-btn:hover { background: var(--accent-hover); }

  .file-path {
    color: var(--text-secondary);
    font-size: 12px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    max-width: 300px;
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
