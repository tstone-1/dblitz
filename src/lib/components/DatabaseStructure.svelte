<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { appState, type ColumnInfo, type SchemaEntry } from "$lib/store.svelte";

  let selectedTable = $state<string | null>(null);
  let columns = $state<ColumnInfo[]>([]);
  let schema = $state<SchemaEntry[]>([]);
  let schemaView = $state(false);

  $effect(() => {
    if (appState.dbPath) {
      loadSchema();
    }
  });

  async function loadSchema() {
    try {
      schema = await invoke<SchemaEntry[]>("get_schema");
    } catch (e) {
      appState.error = String(e);
    }
  }

  async function selectTable(name: string) {
    selectedTable = name;
    try {
      columns = await invoke<ColumnInfo[]>("get_columns", { table: name });
    } catch (e) {
      appState.error = String(e);
    }
  }

  function typeColor(t: string): string {
    const upper = t.toUpperCase();
    if (upper.includes("INT")) return "var(--accent)";
    if (upper.includes("TEXT") || upper.includes("CHAR") || upper.includes("CLOB"))
      return "var(--success)";
    if (upper.includes("REAL") || upper.includes("FLOAT") || upper.includes("DOUBLE"))
      return "var(--warning)";
    if (upper.includes("BLOB")) return "var(--error)";
    return "var(--text-secondary)";
  }
</script>

{#if !appState.dbPath}
  <div class="empty">Open a SQLite database to view its structure.</div>
{:else}
  <div class="structure-layout">
    <div class="toggle-bar">
      <button class:active={!schemaView} onclick={() => (schemaView = false)}>
        Tables & Columns
      </button>
      <button class:active={schemaView} onclick={() => (schemaView = true)}>
        Raw Schema (SQL)
      </button>
    </div>

    {#if schemaView}
      <div class="schema-list">
        {#each schema as entry}
          {#if entry.sql}
            <div class="schema-entry">
              <div class="schema-header">
                <span class="schema-type">{entry.obj_type}</span>
                <span class="schema-name">{entry.name}</span>
              </div>
              <pre class="schema-sql">{entry.sql};</pre>
            </div>
          {/if}
        {/each}
      </div>
    {:else}
      <div class="tables-columns">
        <div class="table-list">
          <div class="section-title">Tables</div>
          {#each appState.tables as table}
            <button
              class="table-item"
              class:selected={selectedTable === table.name}
              onclick={() => selectTable(table.name)}
            >
              <span class="table-name">{table.name}</span>
              <span class="row-count">{table.row_count.toLocaleString()} rows</span>
            </button>
          {/each}
        </div>

        <div class="column-list">
          {#if selectedTable}
            <div class="section-title">Columns in "{selectedTable}"</div>
            <table>
              <thead>
                <tr>
                  <th>#</th>
                  <th>Name</th>
                  <th>Type</th>
                  <th>NOT NULL</th>
                  <th>Default</th>
                  <th>PK</th>
                </tr>
              </thead>
              <tbody>
                {#each columns as col}
                  <tr>
                    <td class="cid">{col.cid}</td>
                    <td class="col-name">{col.name}</td>
                    <td><span class="type-badge" style="color: {typeColor(col.col_type)}">{col.col_type || 'ANY'}</span></td>
                    <td class="center">{col.notnull ? 'YES' : ''}</td>
                    <td class="default-val">{col.default_value ?? ''}</td>
                    <td class="center">{col.pk ? 'PK' : ''}</td>
                  </tr>
                {/each}
              </tbody>
            </table>
          {:else}
            <div class="empty-columns">Select a table to view its columns.</div>
          {/if}
        </div>
      </div>
    {/if}
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

  .structure-layout {
    display: flex;
    flex-direction: column;
    height: 100%;
  }

  .toggle-bar {
    display: flex;
    gap: 4px;
    padding: 8px 12px;
    border-bottom: 1px solid var(--border-color);
  }

  .toggle-bar button {
    font-size: 12px;
    padding: 3px 12px;
  }

  .toggle-bar button.active {
    background: var(--accent);
    color: var(--bg-primary);
  }

  .tables-columns {
    display: flex;
    flex: 1;
    overflow: hidden;
  }

  .table-list {
    width: 260px;
    border-right: 1px solid var(--border-color);
    overflow-y: auto;
    flex-shrink: 0;
  }

  .section-title {
    padding: 8px 12px;
    font-size: 11px;
    font-weight: 600;
    color: var(--text-muted);
    text-transform: uppercase;
    letter-spacing: 0.5px;
  }

  .table-item {
    display: flex;
    justify-content: space-between;
    width: 100%;
    padding: 6px 12px;
    border: none;
    border-radius: 0;
    text-align: left;
    background: transparent;
  }

  .table-item:hover {
    background: var(--bg-hover);
  }

  .table-item.selected {
    background: var(--bg-tertiary);
    border-left: 3px solid var(--accent);
  }

  .table-name {
    font-weight: 500;
  }

  .row-count {
    color: var(--text-muted);
    font-size: 11px;
  }

  .column-list {
    flex: 1;
    overflow: auto;
    padding: 0;
  }

  .cid {
    color: var(--text-muted);
    width: 30px;
  }

  .col-name {
    font-weight: 500;
  }

  .type-badge {
    font-family: 'Cascadia Code', 'Fira Code', monospace;
    font-size: 12px;
  }

  .center {
    text-align: center;
  }

  .default-val {
    color: var(--text-secondary);
    font-family: monospace;
    font-size: 12px;
  }

  .empty-columns {
    display: flex;
    align-items: center;
    justify-content: center;
    height: 100%;
    color: var(--text-muted);
  }

  .schema-list {
    flex: 1;
    overflow-y: auto;
    padding: 12px;
  }

  .schema-entry {
    margin-bottom: 16px;
  }

  .schema-header {
    margin-bottom: 4px;
  }

  .schema-type {
    display: inline-block;
    padding: 1px 6px;
    background: var(--bg-tertiary);
    border-radius: 3px;
    font-size: 10px;
    text-transform: uppercase;
    color: var(--accent);
    margin-right: 8px;
  }

  .schema-name {
    font-weight: 600;
  }

  .schema-sql {
    background: var(--bg-input);
    padding: 8px 12px;
    border-radius: 4px;
    font-family: 'Cascadia Code', 'Fira Code', monospace;
    font-size: 12px;
    white-space: pre-wrap;
    word-break: break-all;
    color: var(--text-secondary);
    border: 1px solid var(--border-color);
  }
</style>
