<script lang="ts">
  import { ui } from "$lib/i18n";

  let {
    zoom,
    onZoom,
    onReset,
  }: {
    zoom: number;
    onZoom: (zoom: number) => void;
    onReset: () => void;
  } = $props();
</script>

<div class="join" aria-label={ui.zoom.controls}>
  <button
    class="btn btn-xs join-item px-2"
    type="button"
    onclick={() => onZoom(zoom - 0.25)}
    disabled={zoom <= 0.5}
    aria-label={ui.zoom.out}
    title={ui.zoom.out}
  >−</button>
  <button
    class="btn btn-xs join-item min-w-14 px-2 font-mono font-normal"
    type="button"
    onclick={onReset}
    aria-label={ui.zoom.reset}
    title={ui.zoom.resetTitle}
  >{Math.round(zoom * 100)}%</button>
  <button
    class="btn btn-xs join-item px-2"
    type="button"
    onclick={() => onZoom(zoom + 0.25)}
    disabled={zoom >= 3}
    aria-label={ui.zoom.in}
    title={ui.zoom.in}
  >+</button>
</div>
