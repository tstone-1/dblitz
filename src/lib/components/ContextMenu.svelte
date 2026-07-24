<script lang="ts">
  import type { Snippet } from "svelte";

  // Shared shell for the app's right-click menus (DataGrid cell/pin/header
  // menus and BrowseData's global-filter pin menu). Owns the fixed backdrop
  // that closes on click/right-click plus the fixed-positioned menu box at
  // x/y; each caller supplies its own .ctx-item markup as children. The
  // .ctx-backdrop/.ctx-menu styling is app-global (app.css), so it applies
  // here without a local <style>.
  interface Props {
    x: number;
    y: number;
    onClose: () => void;
    children: Snippet;
  }

  let { x, y, onClose, children }: Props = $props();
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<!-- svelte-ignore a11y_click_events_have_key_events -->
<div class="ctx-backdrop" onclick={onClose} oncontextmenu={(e) => { e.preventDefault(); onClose(); }}></div>
<div class="ctx-menu" style="left: {x}px; top: {y}px;">
  {@render children()}
</div>
