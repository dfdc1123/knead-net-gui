<script lang="ts">
  let { current = $bindable(0) } = $props();

  const tabs = [
    { label: "源文件", icon: "📁" },
    { label: "面包板", icon: "🍞" },
    { label: "计算", icon: "⚙️" },
    { label: "结果", icon: "✨" },
  ];
</script>

<div class="dock bg-base-200 border-t border-base-300">
  {#each tabs as tab, i}
    <button class={current === i ? "dock-active" : ""} onclick={() => (current = i)}>
      <span class="text-xl">{tab.icon}</span>
      <span class="dock-label">{tab.label}</span>
    </button>
  {/each}
</div>