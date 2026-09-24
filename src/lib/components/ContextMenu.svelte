<script lang="ts">
  import type { Snippet } from "svelte";
  import { placeContextMenu } from "./contextMenuPosition";

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

  let menu: HTMLDivElement | undefined = $state();

  // x/y is the pointer; the menu's real size is only known once it has
  // rendered, so the final position is written here, after layout and before
  // paint. Plain style writes, not state, so this cannot re-trigger itself.
  $effect(() => {
    if (!menu) return;
    const rect = menu.getBoundingClientRect();
    const pos = placeContextMenu(
      { x, y },
      { width: rect.width, height: rect.height },
      { width: window.innerWidth, height: window.innerHeight },
    );
    menu.style.left = `${pos.x}px`;
    menu.style.top = `${pos.y}px`;
  });
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<!-- svelte-ignore a11y_click_events_have_key_events -->
<div class="ctx-backdrop" onclick={onClose} oncontextmenu={(e) => { e.preventDefault(); onClose(); }}></div>
<div class="ctx-menu" bind:this={menu} style="left: {x}px; top: {y}px;">
  {@render children()}
</div>
