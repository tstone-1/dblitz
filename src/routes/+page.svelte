<script lang="ts">
  import "../app.css";
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { appState, initTheme, openDatabase, closeDatabase } from "$lib/store.svelte";
  import Toolbar from "$lib/components/Toolbar.svelte";
  import DatabaseStructure from "$lib/components/DatabaseStructure.svelte";
  import BrowseData from "$lib/components/BrowseData.svelte";
  import ExecuteSQL from "$lib/components/ExecuteSQL.svelte";

  async function handleOpenFile(path: string) {
    if (appState.dbPath) await closeDatabase();
    await openDatabase(path);
    appState.activeTab = "browse";
  }

  onMount(() => {
    initTheme();

    // Open file passed via CLI args (file association / jump list launch)
    invoke<string | null>("get_initial_file").then((path) => {
      if (path) handleOpenFile(path);
    });

    function onKeyDown(e: KeyboardEvent) {
      if (e.key === "F12") {
        e.preventDefault();
        invoke("toggle_devtools");
      }
    }
    document.addEventListener("keydown", onKeyDown);

    return () => {
      document.removeEventListener("keydown", onKeyDown);
    };
  });
</script>

<div class="app-shell">
  <Toolbar />
  <div class="content">
    <div class="tab-panel" class:hidden={appState.activeTab !== "structure"}>
      <DatabaseStructure />
    </div>
    <div class="tab-panel" class:hidden={appState.activeTab !== "browse"}>
      <BrowseData />
    </div>
    <div class="tab-panel" class:hidden={appState.activeTab !== "sql"}>
      <ExecuteSQL />
    </div>
  </div>
</div>

<style>
  .app-shell {
    display: flex;
    flex-direction: column;
    height: 100vh;
    overflow: hidden;
  }

  .content {
    flex: 1;
    overflow: hidden;
  }

  .tab-panel {
    height: 100%;
  }

  .tab-panel.hidden {
    display: none;
  }
</style>
