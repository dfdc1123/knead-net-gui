<script lang="ts">
  let {
    zoom,
    onZoom,
  }: {
    zoom: number;
    onZoom: (zoom: number) => void;
  } = $props();
</script>

<div class="join" aria-label="缩放控制">
  <button
    class="btn btn-xs join-item px-2"
    type="button"
    onclick={() => onZoom(zoom - 0.25)}
    disabled={zoom <= 0.5}
    aria-label="缩小"
    title="缩小"
  >−</button>
  <button
    class="btn btn-xs join-item min-w-14 px-2 font-mono font-normal"
    type="button"
    onclick={() => onZoom(1)}
    aria-label="重置缩放"
    title="重置为 100%"
  >{Math.round(zoom * 100)}%</button>
  <button
    class="btn btn-xs join-item px-2"
    type="button"
    onclick={() => onZoom(zoom + 0.25)}
    disabled={zoom >= 3}
    aria-label="放大"
    title="放大"
  >+</button>
</div>
