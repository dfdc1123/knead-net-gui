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
  let nextStep = $derived(
    current < tabs.length - 1 && enabled[current + 1] ? current + 1 : -1,
  );
</script>

<nav class="dock dock-sm z-50 border-t border-base-300 bg-base-100" aria-label={ui.dock.aria}>
  {#each tabs as tab, i}
    <button
      class:dock-active={current === i}
      class:text-primary={current === i}
      onclick={() => (current = i)}
      disabled={!enabled[i]}
      aria-current={current === i ? "step" : undefined}
      aria-label={nextStep === i
        ? ui.dock.nextStep(tab.label)
        : enabled[i]
          ? tab.label
          : ui.dock.unavailable(tab.label)}
      title={nextStep === i
        ? ui.dock.nextStep(tab.label)
        : enabled[i]
          ? tab.label
          : ui.dock.prerequisite}
    >
      {#if nextStep === i}
        <span class="aura aura-sm workflow-next-step text-primary" aria-hidden="true">
          <span class="flex h-6 items-center gap-1.5 rounded-field bg-primary px-2 text-primary-content shadow-sm">
            <span class="font-mono text-sm font-bold">{i + 1}</span>
            <span class="text-[0.625rem] font-semibold">{ui.dock.nextHint}</span>
          </span>
        </span>
      {:else}
        <span class="font-mono text-base font-bold">{i + 1}</span>
      {/if}
      <span class="dock-label">{tab.label}</span>
    </button>
  {/each}
</nav>
