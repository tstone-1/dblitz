<script lang="ts">
  import {
    executeSql as executeSqlCmd,
    cancelQueries,
    exportToXlsx,
    type SqlResult,
  } from "$lib/ipc";
  import {
    appState,
    getTableConfig,
    persistSqlHistory,
  } from "$lib/store.svelte";
  import DataGrid from "./DataGrid.svelte";
  import type { SelectionData } from "./selectionData";
  import SqlEditor from "./SqlEditor.svelte";
  import { resolveResultColumnColors } from "./sqlTable";

  let sql = $state("");
  let result = $state<SqlResult | null>(null);
  // The query that produced `result` — the editor `sql` may change afterwards,
  // so color lookup must key off what actually ran, not the current text.
  let executedSql = $state("");
  let running = $state(false);
  let showHistory = $state(false);

  // Schema for autocomplete: { tableName: [col1, col2, ...] }
  let sqlSchema = $derived(appState.tableColumns);

  // Reuse the per-table column colors from Browse Data for result columns that
  // came from the query's primary (FROM) table. The resolution logic lives in
  // `resolveResultColumnColors` (pure + tested); this derived just feeds it
  // reactive state so a recolor in Browse Data updates the SQL grid live.
  let resultColumnColors = $derived.by<Record<string, string>>(() =>
    result
      ? resolveResultColumnColors({
          sql: executedSql,
          columns: result.columns,
          tableNames: appState.tables.map((t) => t.name),
          getColumnColors: (t) => getTableConfig(t).column_colors,
        })
      : {},
  );

  // Clear results on a database-session change (dbOpenGeneration bumps on every
  // successful open, same-path reopen included). Without this the SQL tab keeps
  // showing the previous database's rows under the new DB's context, and
  // resultColumnColors re-resolves the stale executedSql against the new DB's
  // tables. The typed `sql` text is deliberately KEPT -- re-running the same
  // query against the just-opened database is a natural next step; only the
  // executed result and the query it was resolved against are dropped.
  let prevDbGen = 0;
  $effect(() => {
    const gen = appState.dbOpenGeneration;
    if (gen !== prevDbGen) {
      prevDbGen = gen;
      result = null;
      executedSql = "";
    }
  });

  async function executeSql() {
    // Re-entrancy guard: the CodeMirror Ctrl+Enter keymap calls onexecute
    // unconditionally, so without this a second Ctrl+Enter (or Enter while the
    // button is disabled) would fire a concurrent invoke and a duplicate
    // history entry. Ctrl+Enter while running is simply ignored (not a cancel).
    if (running) return;
    const trimmed = sql.trim();
    if (!trimmed) return;

    running = true;
    result = null;
    try {
      result = await executeSqlCmd(trimmed);
      executedSql = trimmed;

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
        column_types: [],
        error: String(e),
        truncated: false,
      };
    } finally {
      running = false;
    }
  }

  async function cancelExecution() {
    // The backend cancel_queries command flips the cancellation token for the
    // in-flight statement; execute_sql then returns whatever rows it had
    // fetched so far (partial results this component already renders). `running`
    // clears in executeSql's finally when that resolves.
    try {
      await cancelQueries();
    } catch (e) {
      appState.error = String(e);
    }
  }

  function loadFromHistory(entry: { sql: string }) {
    sql = entry.sql;
    // Panel stays open so the user can keep browsing/comparing entries.
  }

  function loadAndRunFromHistory(entry: { sql: string }) {
    sql = entry.sql;
    executeSql();
  }

  function formatTime(ts: number): string {
    return new Date(ts).toLocaleTimeString();
  }

  async function exportSelection(data: SelectionData) {
    // A selection's headers are a contiguous slice of `result.columns` (see
    // buildSelectionData), so walk `result.columns` forward matching each
    // header by name to find its declared type - the forward-only search
    // keeps duplicate-named result columns (e.g. a self-join) aligned to the
    // right occurrence instead of always the first.
    const columns = result?.columns ?? [];
    const types = result?.column_types ?? [];
    let searchFrom = 0;
    const columnTypes = data.headers.map((header) => {
      const idx = columns.indexOf(header, searchFrom);
      if (idx === -1) return "";
      searchFrom = idx + 1;
      return types[idx] ?? "";
    });
    await exportToXlsx({
      headers: data.headers,
      rows: data.rows,
      columnTypes,
    });
  }
</script>

{#if !appState.dbPath}
  <div class="empty">Open a SQLite database to execute SQL.</div>
{:else}
  <div class="sql-layout">
    <div class="editor-area">
      <div class="editor-header">
        <span class="hint">Read-only — Ctrl+Enter to execute</span>
        <button
          onclick={() => (showHistory = !showHistory)}
          class="history-btn"
        >
          History ({appState.sqlHistory.length})
        </button>
        {#if running}
          <button onclick={cancelExecution} class="run-btn cancel-btn">
            Cancel
          </button>
        {:else}
          <button onclick={executeSql} class="run-btn" disabled={!sql.trim()}>
            Execute
          </button>
        {/if}
      </div>
      <SqlEditor
        bind:value={sql}
        onexecute={executeSql}
        schema={sqlSchema}
        placeholder="Enter a SELECT query (read-only)..."
      />
    </div>

    {#if showHistory}
      <div class="history-panel">
        <div class="history-title">
          Query History <span class="history-hint">— click to load, double-click to run</span>
        </div>
        {#if appState.sqlHistory.length === 0}
          <div class="history-empty">No queries yet.</div>
        {:else}
          {#each appState.sqlHistory as entry}
            <button
              class="history-entry"
              class:error={entry.error}
              title="Click to load into editor — double-click to load and run"
              onclick={() => loadFromHistory(entry)}
              ondblclick={() => loadAndRunFromHistory(entry)}
            >
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
        {/if}
        {#if !result.error}
          <div class="result-info">
            {#if result.rows.length > 0}
              {result.rows.length} row{result.rows.length !== 1 ? 's' : ''} returned
            {:else}
              Query executed successfully
            {/if}
          </div>
          {#if result.truncated}
            <div class="result-warning">
              Showing the first {result.rows.length.toLocaleString()} rows (result
              cap) — narrow your query or page with OFFSET to see more.
            </div>
          {/if}
        {:else if result.rows.length > 0}
          <!-- A mid-iteration SQL error still leaves already-fetched rows -
               show them below the error banner instead of hiding them. -->
          <div class="result-warning">
            Showing the {result.rows.length.toLocaleString()} row{result.rows.length !== 1 ? 's' : ''}
            fetched before the error above — results are partial.
          </div>
        {/if}
        {#if result.columns.length > 0 && (!result.error || result.rows.length > 0)}
          <DataGrid
            columns={result.columns}
            mode={{ kind: "static", rows: result.rows }}
            columnColors={resultColumnColors}
            onExport={exportSelection}
            onNotice={(message) => (appState.notice = message)}
            onError={(message) => (appState.error = message)}
          />
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

  .cancel-btn {
    background: var(--error);
  }

  .cancel-btn:hover {
    background: var(--error);
    opacity: 0.85;
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

  .history-hint {
    font-weight: 400;
    text-transform: none;
    opacity: 0.75;
    margin-left: 4px;
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

  .result-warning {
    padding: 4px 8px;
    background: rgba(249, 226, 175, 0.15);
    color: var(--warning);
    font-size: 12px;
    border-bottom: 1px solid var(--border-color);
    flex-shrink: 0;
  }
</style>
