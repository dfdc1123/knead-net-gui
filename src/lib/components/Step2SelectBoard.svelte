<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import type { BreadboardPreset, BreadboardSelection } from "$lib/layout";
  import BreadboardPreview from "./BreadboardPreview.svelte";

  let {
    onStatusChange = () => {},
    onBoardChange = () => {},
  }: {
    onStatusChange?: (ready: boolean) => void;
    onBoardChange?: (board: BreadboardSelection | null) => void;
  } = $props();

  type Info = { preset: string; cols: number; holes: number; has_power_rails: boolean };

  const PRESETS: { id: BreadboardPreset; name: string; desc: string; defaultCols: number; rail: boolean }[] = [
    { id: "hole170", name: "170 孔", desc: "迷你 17×10 main", defaultCols: 17, rail: false },
    { id: "hole400", name: "400 孔", desc: "标准 30×10 main + 电源轨", defaultCols: 30, rail: true },
    { id: "hole800", name: "800 孔", desc: "加宽 63×10 main + 宽电源轨", defaultCols: 63, rail: true },
  ];

  let preset = $state<BreadboardPreset>("hole400");
  let cols = $state(30);
  let info = $state<Info | null>(null);
  let busy = $state(false);
  let error = $state("");

  function pick(p: BreadboardPreset) {
    if (busy) return;
    preset = p;
    cols = PRESETS.find((x) => x.id === p)!.defaultCols;
  }

  // cols 变化 → 自动重提交 (debounce 250ms)
  let timer: ReturnType<typeof setTimeout> | null = null;
  $effect(() => {
    cols; preset;
    if (timer) clearTimeout(timer);
    timer = setTimeout(() => submit(preset, cols), 250);
  });

  async function submit(p: BreadboardPreset, c: number) {
    busy = true;
    error = "";
    onStatusChange(false);
    try {
      info = await invoke<Info>("set_breadboard", { preset: p, cols: c });
      onBoardChange({ preset: p, cols: info.cols });
      onStatusChange(true);
    } catch (e) {
      info = null;
      onBoardChange(null);
      error = String(e);
    } finally {
      busy = false;
    }
  }
</script>

<div class="h-full flex flex-col gap-4 p-6 overflow-auto">
  <h2 class="text-xs font-semibold uppercase tracking-wider text-base-content/50">选择面包板</h2>

  <div class="grid grid-cols-3 gap-3">
    {#each PRESETS as p}
      <button
        class="card bg-base-200 hover:bg-base-300 transition-colors text-left p-4 cursor-pointer
               {preset === p.id ? 'ring-2 ring-primary' : ''}"
        onclick={() => pick(p.id)}
        disabled={busy}
      >
        <div class="text-2xl font-bold">{p.name}</div>
        <div class="text-xs text-base-content/60 mt-1">{p.desc}</div>
        <div class="text-[10px] text-base-content/40 mt-2">默认 {p.defaultCols} 列</div>
      </button>
    {/each}
  </div>

  <div class="flex items-center gap-3 bg-base-200 rounded p-3">
    <span class="text-sm">列数 (cols)</span>
    <input
      type="number"
      class="input input-sm input-bordered w-24"
      min="3"
      max="120"
      bind:value={cols}
    />
    {#if info}
      <div class="flex gap-2 ml-auto">
        <span class="badge badge-primary badge-sm">{info.holes} 孔</span>
        {#if info.has_power_rails}
          <span class="badge badge-secondary badge-sm">含电源轨</span>
        {:else}
          <span class="badge badge-ghost badge-sm">无电源轨</span>
        {/if}
      </div>
    {/if}
  </div>

  {#if error}
    <div class="alert alert-error text-sm">{error}</div>
  {/if}

  {#if info}
    <div class="card bg-base-200 p-4">
      <div class="mb-3 flex items-center justify-between gap-3">
        <h3 class="text-xs font-semibold uppercase tracking-wider text-base-content/50">
          预览 · 主区 {info.cols} × 10
        </h3>
        <span class="text-[10px] text-base-content/40">视觉比例参考真实板型</span>
      </div>
      <div class="overflow-auto rounded-box bg-base-100">
        <BreadboardPreview {preset} cols={info.cols} />
      </div>
    </div>
  {/if}
</div>
