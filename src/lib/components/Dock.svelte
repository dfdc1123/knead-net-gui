<script lang="ts">
  let {
    current = $bindable(0),
    enabled = [true, false, false, false],
  }: {
    current?: number;
    enabled?: boolean[];
  } = $props();

  const tabs = [
    { label: "源文件", icon: "📁" },
    { label: "面包板", icon: "🍞" },
    { label: "计算", icon: "⚙️" },
    { label: "结果", icon: "✨" },
  ];
</script>

<div class="dock bg-base-200 border-t border-base-300">
  {#each tabs as tab, i}
    <button
      class={current === i ? "dock-active" : ""}
      onclick={() => (current = i)}
      disabled={!enabled[i]}
      aria-label={enabled[i] ? tab.label : `${tab.label}（请先完成前置步骤）`}
      title={enabled[i] ? tab.label : "请先完成前置步骤"}
    >
      <span class="text-xl">{tab.icon}</span>
      <span class="dock-label">{tab.label}</span>
    </button>
  {/each}
</div>
