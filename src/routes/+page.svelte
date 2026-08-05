<script lang="ts">
  import "../app.css";
  import { dev } from "$app/environment";
  import { onMount } from "svelte";
  import { listen, type UnlistenFn } from "@tauri-apps/api/event";
  import { getInitialFile, toggleDevtools } from "$lib/ipc";
  import { appState, initTheme, openDatabase } from "$lib/store.svelte";
  import { update } from "$lib/updateState.svelte";
  import Toolbar from "$lib/components/Toolbar.svelte";
  import AppBanners from "$lib/components/AppBanners.svelte";
  import DatabaseStructure from "$lib/components/DatabaseStructure.svelte";
  import BrowseData from "$lib/components/BrowseData.svelte";
  import ExecuteSQL from "$lib/components/ExecuteSQL.svelte";

  // Opening a database while another one is open needs no explicit close, and
  // this path deliberately does not do one: the backend swaps the connection
  // atomically under the conn lock, and appState.dbOpenGeneration bumps on
  // every successful open, which is what drives every frontend reset. A
  // close-first only added a second loading cycle and made the close read as
  // load-bearing. Toolbar's "Open DB" and the recents list already call
  // openDatabase() straight; this keeps the OS-open route (Finder
  // double-click, Dock "Open Recent", CLI arg) identical to them.
  async function handleOpenFile(path: string) {
    await openDatabase(path);
    appState.activeTab = "browse";
  }

  onMount(() => {
    initTheme();

    // macOS delivers a document open (Finder double-click, Dock "Open Recent",
    // `open -a dblitz file.db`) as an Apple event that reaches the backend as
    // RunEvent::Opened -- never as a CLI arg. Two things have to be true for
    // none of those to get lost, and they are why this is one awaited sequence
    // rather than two independent calls:
    //   1. the listener is registered BEFORE get_initial_file, because that
    //      call is what tells the backend an emit would now be heard;
    //   2. get_initial_file drains a path the backend stashed because it
    //      arrived before this webview existed at all (the cold-launch case,
    //      and also the plain CLI-arg / jump-list launch on Windows).
    // The backend picks exactly one of the two routes per request, so a path
    // arriving mid-mount is opened once, not twice.
    let unlistenOpenFile: UnlistenFn | undefined;
    let disposed = false;
    void (async () => {
      const unlisten = await listen<string>("open-file", (event) => {
        void handleOpenFile(event.payload);
      });
      if (disposed) {
        unlisten();
        return;
      }
      unlistenOpenFile = unlisten;
      const path = await getInitialFile();
      if (path) await handleOpenFile(path);
    })();

    // Resolves the running version, whether this launch follows an update, and
    // whether this install can replace itself at all.
    void update.loadStatus();
    void update.loadStartupPreference();
    // Fires ~10s from now and re-reads the opt-out at that point, not here: the
    // preference load above is asynchronous, so a snapshot taken now would still
    // see the default and check for a user who had turned that off.
    const teardownUpdateCheck = update.scheduleStartupCheck();

    function onKeyDown(e: KeyboardEvent) {
      if (dev && e.key === "F12") {
        e.preventDefault();
        toggleDevtools();
      }
    }
    document.addEventListener("keydown", onKeyDown);

    // Suppress the native browser context menu (back/refresh/save as/print/...)
    // which adds no value in a desktop viewer. Editable fields keep it so users
    // still get cut/copy/paste/select-all; the app's own custom menus (grid
    // cells, headers, filter pins) call preventDefault themselves and so still
    // open normally before this listener runs.
    function onContextMenu(e: MouseEvent) {
      const target = e.target as HTMLElement | null;
      if (target?.closest("input, textarea, [contenteditable='true']")) return;
      e.preventDefault();
    }
    document.addEventListener("contextmenu", onContextMenu);

    return () => {
      disposed = true;
      unlistenOpenFile?.();
      document.removeEventListener("keydown", onKeyDown);
      document.removeEventListener("contextmenu", onContextMenu);
      teardownUpdateCheck();
    };
  });
</script>

<div class="app-shell">
  <Toolbar />
  <!-- App-global error/notice/update bars: they sit between the toolbar and the
       tab content, so the order is decided here rather than inside Toolbar. -->
  <AppBanners />
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
