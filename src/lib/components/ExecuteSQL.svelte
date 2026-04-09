<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { appState, persistSqlHistory, type SqlResult } from "$lib/store.svelte";
  import DataGrid from "./DataGrid.svelte";
  import SqlEditor from "./SqlEditor.svelte";

  let sql = $state("");
  let result = $state<SqlResult | null>(null);
  let running = $state(false);
  let showHistory = $state(false);

  // Schema for autocomplete: { tableName: [col1, col2, ...] }
  let sqlSchema = $derived(appState.tableColumns);

  async function executeSql() {
    const trimmed = sql.trim();
    if (!trimmed) return;

    running = true;
    result = null;
    try {
      result = await invoke<SqlResult>("execute_sql", { sql: trimmed });

      appState.sqlHistory = [
        {
          sql: trimmed,
          timestamp: Date.now(),
          error: !!result.error,
        },
        ...appState.sqlHistory.slice(0, 99),
      ];
      persistSqlHistory();
    } catch (e) {
      result = {
        columns: [],
        rows: [],
        rows_affected: 0,
        error: String(e),
      };
    } finally {
      running = false;
    }
  }

  function loadFromHistory(entry: { sql: string }) {
    sql = entry.sql;
    showHistory = false;
  }

  function formatTime(ts: number): string {
    return new Date(ts).toLocaleTimeString();
  }
</script>

{#if !appState.dbPath}
  <div class="empty">Open a SQLite database to execute SQL.</div>
{:else}
  <div class="sql-layout">
    <div class="editor-area">
      <div class="editor-header">
        <span class="hint">Ctrl+Enter to execute</span>
        <button
          onclick={() => (showHistory = !showHistory)}
          class="history-btn"
        >
          History ({appState.sqlHistory.length})
        </button>
        <button onclick={executeSql} class="run-btn" disabled={running || !sql.trim()}>
          {running ? "Running..." : "Execute"}
        </button>
      </div>
      <SqlEditor
        bind:value={sql}
        onexecute={executeSql}
        schema={sqlSchema}
        placeholder="Enter SQL query..."
      />
    </div>

    {#if showHistory}
      <div class="history-panel">
        <div class="history-title">Query History</div>
        {#if appState.sqlHistory.length === 0}
          <div class="history-empty">No queries yet.</div>
        {:else}
          {#each appState.sqlHistory as entry}
            <button class="history-entry" class:error={entry.error} onclick={() => loadFromHistory(entry)}>
              <span class="history-time">{formatTime(entry.timestamp)}</span>
              <span class="history-sql">{entry.sql}</span>
            </button>
          {/each}
        {/if}
      </div>
    {/if}

    <div class="result-area">
      {#if result}
        {#if result.error}
          <div class="result-error">{result.error}</div>
        {:else}
          <div class="result-info">
            {#if result.rows.length > 0}
              {result.rows.length} row{result.rows.length !== 1 ? 's' : ''} returned
            {:else if result.rows_affected > 0}
              {result.rows_affected} row{result.rows_affected !== 1 ? 's' : ''} affected
            {:else}
              Query executed successfully
            {/if}
          </div>
          {#if result.columns.length > 0}
            <DataGrid
              columns={result.columns}
              rows={result.rows}
            />
          {/if}
        {/if}
      {/if}
    </div>
  </div>
{/if}

<style>
  .empty {
    display: flex;
    align-items: center;
    justify-content: center;
    height: 100%;
    color: var(--text-muted);
    font-size: 14px;
  }

  .sql-layout {
    display: flex;
    flex-direction: column;
    height: 100%;
    overflow: hidden;
  }

  .editor-area {
    display: flex;
    flex-direction: column;
    flex-shrink: 0;
  }

  .editor-header {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 6px 8px;
    border-bottom: 1px solid var(--border-color);
  }

  .hint {
    color: var(--text-muted);
    font-size: 11px;
    margin-right: auto;
  }

  .history-btn {
    font-size: 12px;
    padding: 3px 10px;
  }

  .run-btn {
    background: var(--accent);
    color: var(--bg-primary);
    font-weight: 600;
    border: none;
    padding: 4px 16px;
  }

  .run-btn:hover {
    background: var(--accent-hover);
  }

  .run-btn:disabled {
    opacity: 0.5;
    cursor: default;
  }

  .history-panel {
    max-height: 200px;
    overflow-y: auto;
    border-bottom: 1px solid var(--border-color);
    background: var(--bg-secondary);
    flex-shrink: 0;
  }

  .history-title {
    padding: 6px 8px;
    font-size: 11px;
    font-weight: 600;
    color: var(--text-muted);
    text-transform: uppercase;
    border-bottom: 1px solid var(--border-color);
  }

  .history-empty {
    padding: 12px;
    color: var(--text-muted);
    font-size: 12px;
  }

  .history-entry {
    display: flex;
    gap: 8px;
    width: 100%;
    padding: 4px 8px;
    border: none;
    border-radius: 0;
    text-align: left;
    background: transparent;
    font-size: 12px;
    border-bottom: 1px solid var(--border-color);
  }

  .history-entry:hover {
    background: var(--bg-hover);
  }

  .history-entry.error {
    border-left: 3px solid var(--error);
  }

  .history-time {
    color: var(--text-muted);
    font-size: 10px;
    flex-shrink: 0;
    width: 70px;
  }

  .history-sql {
    font-family: monospace;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .result-area {
    flex: 1;
    overflow: hidden;
    display: flex;
    flex-direction: column;
  }

  .result-error {
    padding: 8px 12px;
    background: rgba(243, 139, 168, 0.15);
    color: var(--error);
    font-family: monospace;
    font-size: 12px;
    border-bottom: 1px solid var(--error);
  }

  .result-info {
    padding: 4px 8px;
    color: var(--success);
    font-size: 12px;
    border-bottom: 1px solid var(--border-color);
    flex-shrink: 0;
  }
</style>
