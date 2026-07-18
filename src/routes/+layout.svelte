<script lang="ts">
  import "../app.css";
  let { children } = $props();

  function preventContextMenu(event: MouseEvent) {
    event.preventDefault();
  }
</script>

<svelte:window oncontextmenu={preventContextMenu} />

{@render children()}
