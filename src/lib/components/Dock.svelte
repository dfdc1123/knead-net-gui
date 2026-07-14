<script lang="ts">
  let {
    current = $bindable(0),
    enabled = [true, false, false, false],
  }: {
    current?: number;
    enabled?: boolean[];
  } = $props();

  const tabs = [
    { label: "工程" },
    { label: "面包板" },
    { label: "计算" },
    { label: "结果" },
  ];
</script>

<nav class="dock dock-sm z-50 border-t border-base-300 bg-base-100" aria-label="布局流程">
  {#each tabs as tab, i}
    <button
      class:dock-active={current === i}
      class:text-primary={current === i}
      onclick={() => (current = i)}
      disabled={!enabled[i]}
      aria-current={current === i ? "step" : undefined}
      aria-label={enabled[i] ? tab.label : `${tab.label}（请先完成前置步骤）`}
      title={enabled[i] ? tab.label : "请先完成前置步骤"}
    >
      <span class="font-mono text-base font-bold">{i + 1}</span>
      <span class="dock-label">{tab.label}</span>
    </button>
  {/each}
</nav>
