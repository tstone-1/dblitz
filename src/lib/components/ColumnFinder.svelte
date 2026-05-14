<script lang="ts">
  import { tick } from "svelte";

  interface Props {
    columns: string[];
    hiddenColumns: string[];
    open: boolean;
    onClose: () => void;
    onLocate: (col: string) => void;
  }

  let { columns, hiddenColumns, open, onClose, onLocate }: Props = $props();

  const MAX_RESULTS = 50;

  let query = $state("");
  let selectedIdx = $state(0);
  let inputEl = $state<HTMLInputElement | undefined>();
  let listEl = $state<HTMLDivElement | undefined>();

  // Reset and focus when opened
  $effect(() => {
    if (open) {
      query = "";
      selectedIdx = 0;
      tick().then(() => inputEl?.focus());
    }
  });

  let hiddenSet = $derived(new Set(hiddenColumns));

  let matches = $derived.by(() => {
    const q = query.trim().toLowerCase();
    if (!q) return columns.slice(0, MAX_RESULTS);
    return columns.filter((c) => c.toLowerCase().includes(q)).slice(0, MAX_RESULTS);
  });

  // Clamp selection if matches shrink
  $effect(() => {
    void matches;
    if (selectedIdx >= matches.length) selectedIdx = Math.max(0, matches.length - 1);
  });

  // Keep the selected row visible inside the result list
  $effect(() => {
    void selectedIdx;
    if (!listEl) return;
    const el = listEl.querySelector<HTMLElement>(`[data-i="${selectedIdx}"]`);
    el?.scrollIntoView({ block: "nearest" });
  });

  function commit(col: string) {
    onLocate(col);
    onClose();
  }

  function onKey(e: KeyboardEvent) {
    if (e.key === "Escape") {
      e.preventDefault();
      onClose();
    } else if (e.key === "ArrowDown") {
      e.preventDefault();
      if (matches.length > 0) selectedIdx = (selectedIdx + 1) % matches.length;
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      if (matches.length > 0) selectedIdx = (selectedIdx - 1 + matches.length) % matches.length;
    } else if (e.key === "Enter") {
      e.preventDefault();
      if (matches.length > 0) commit(matches[selectedIdx]);
    } else if (e.key === "Home") {
      e.preventDefault();
      selectedIdx = 0;
    } else if (e.key === "End") {
      e.preventDefault();
      selectedIdx = Math.max(0, matches.length - 1);
    }
  }

  // Split a name around the matched substring for highlighting
  function parts(name: string, q: string): [string, string, string] | null {
    if (!q) return null;
    const idx = name.toLowerCase().indexOf(q.toLowerCase());
    if (idx < 0) return null;
    return [name.slice(0, idx), name.slice(idx, idx + q.length), name.slice(idx + q.length)];
  }
</script>

{#if open}
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <div class="finder-backdrop" onclick={onClose}></div>
  <div class="finder" role="dialog" aria-label="Find column">
    <input
      bind:this={inputEl}
      bind:value={query}
      onkeydown={onKey}
      class="finder-input"
      type="text"
      placeholder="Find column..."
      spellcheck="false"
      autocomplete="off"
    />
    <div class="finder-meta">
      {#if query.trim()}
        {matches.length} match{matches.length === 1 ? "" : "es"}
      {:else}
        {columns.length} columns
      {/if}
      <span class="finder-hint">&uarr;&darr; navigate &middot; Enter locate &middot; Esc close</span>
    </div>
    <div class="finder-list" bind:this={listEl}>
      {#each matches as col, i (col)}
        {@const p = parts(col, query.trim())}
        <button
          type="button"
          class="finder-item"
          class:selected={i === selectedIdx}
          data-i={i}
          onmousemove={() => (selectedIdx = i)}
          onclick={() => commit(col)}
        >
          <span class="finder-name">
            {#if p}{p[0]}<mark>{p[1]}</mark>{p[2]}{:else}{col}{/if}
          </span>
          {#if hiddenSet.has(col)}
            <span class="finder-badge" title="Hidden column — locating will unhide it">hidden</span>
          {/if}
        </button>
      {:else}
        <div class="finder-empty">No matching columns.</div>
      {/each}
    </div>
  </div>
{/if}

<style>
  .finder-backdrop {
    position: fixed;
    inset: 0;
    z-index: 60;
    background: transparent;
  }
  .finder {
    position: absolute;
    top: 6px;
    right: 8px;
    z-index: 61;
    width: 320px;
    background: var(--bg-secondary);
    border: 1px solid var(--border-color);
    border-radius: 6px;
    box-shadow: 0 6px 24px rgba(0, 0, 0, 0.25);
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }
  .finder-input {
    border: none;
    border-bottom: 1px solid var(--border-color);
    background: var(--bg-primary);
    color: var(--text-primary);
    padding: 6px 10px;
    font-size: 12px;
    outline: none;
  }
  .finder-input:focus {
    border-bottom-color: var(--accent);
  }
  .finder-meta {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 3px 10px;
    font-size: 10px;
    color: var(--text-muted);
    background: var(--bg-tertiary);
    border-bottom: 1px solid var(--border-color);
  }
  .finder-hint {
    font-size: 10px;
  }
  .finder-list {
    max-height: 320px;
    overflow-y: auto;
    padding: 2px 0;
  }
  .finder-item {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    width: 100%;
    padding: 4px 10px;
    border: none;
    background: transparent;
    color: var(--text-primary);
    text-align: left;
    font-size: 12px;
    font-family: 'Cascadia Code', 'Cascadia Mono', 'Fira Code', 'Consolas', monospace;
    cursor: pointer;
  }
  .finder-item.selected {
    background: var(--bg-hover);
    box-shadow: inset 2px 0 0 var(--accent);
  }
  .finder-name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    flex: 1;
    min-width: 0;
  }
  .finder-name mark {
    background: color-mix(in srgb, var(--accent) 35%, transparent);
    color: inherit;
    padding: 0 1px;
    border-radius: 2px;
  }
  .finder-badge {
    font-size: 10px;
    color: var(--text-muted);
    background: var(--bg-tertiary);
    border: 1px solid var(--border-color);
    border-radius: 3px;
    padding: 0 4px;
    flex-shrink: 0;
  }
  .finder-empty {
    padding: 12px;
    text-align: center;
    font-size: 11px;
    color: var(--text-muted);
  }
</style>
