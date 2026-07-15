<script lang="ts">
  import { ui } from "$lib/i18n";

  let {
    current = $bindable(0),
    enabled = [true, false, false, false],
  }: {
    current?: number;
    enabled?: boolean[];
  } = $props();

  const tabs = ui.dock.tabs.map((label) => ({ label }));
</script>

<nav class="dock dock-sm z-50 border-t border-base-300 bg-base-100" aria-label={ui.dock.aria}>
  {#each tabs as tab, i}
    <button
      class:dock-active={current === i}
      class:text-primary={current === i}
      onclick={() => (current = i)}
      disabled={!enabled[i]}
      aria-current={current === i ? "step" : undefined}
      aria-label={enabled[i] ? tab.label : ui.dock.unavailable(tab.label)}
      title={enabled[i] ? tab.label : ui.dock.prerequisite}
    >
      <span class="font-mono text-base font-bold">{i + 1}</span>
      <span class="dock-label">{tab.label}</span>
    </button>
  {/each}
</nav>
