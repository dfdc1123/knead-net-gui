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

  const PRESETS: { id: BreadboardPreset; name: string; defaultCols: number }[] = [
    { id: "hole170", name: "170 孔", defaultCols: 17 },
    { id: "hole400", name: "400 孔", defaultCols: 30 },
    { id: "hole800", name: "800 孔", defaultCols: 63 },
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

<div class="mx-auto flex h-full w-full max-w-screen-2xl flex-col gap-4 overflow-hidden p-6">
  <header class="shrink-0">
    <h1 class="text-2xl font-bold">选择面包板</h1>
  </header>

  <div class="grid min-h-0 flex-1 grid-cols-[22rem_minmax(0,1fr)] gap-4">
    <aside class="card min-h-0 border border-base-300 bg-base-100 shadow-sm">
      <div class="card-body gap-4 p-4">
        <fieldset class="fieldset" disabled={busy}>
          <legend class="fieldset-legend">板型</legend>
          <div class="join join-vertical w-full">
            {#each PRESETS as p}
              <label class="join-item flex cursor-pointer items-center gap-3 border border-base-300 px-4 py-3 hover:bg-base-200" class:bg-base-200={preset === p.id}>
                <input
                  type="radio"
                  class="radio radio-primary radio-sm"
                  name="breadboard-preset"
                  checked={preset === p.id}
                  onchange={() => pick(p.id)}
                  aria-label={`选择 ${p.name}`}
                />
                <span class="flex-1 font-semibold">{p.name}</span>
                <span class="badge badge-ghost badge-sm">{p.defaultCols} 列</span>
              </label>
            {/each}
          </div>
        </fieldset>

        <fieldset class="fieldset">
          <legend class="fieldset-legend">列数</legend>
          <label class="input w-full">
            <input type="number" min="3" max="120" bind:value={cols} aria-label="面包板可用列数" />
            <span class="label">3–120</span>
          </label>
        </fieldset>

        {#if info}
          <div class="flex flex-wrap gap-2">
            <span class="badge badge-primary">{info.holes} 孔</span>
            <span class="badge badge-outline">{info.has_power_rails ? "含电源轨" : "无电源轨"}</span>
          </div>
        {/if}

        {#if error}
          <div class="alert alert-error text-sm" role="alert"><span>{error}</span></div>
        {/if}
      </div>
    </aside>

    <section class="card min-h-0 border border-base-300 bg-base-100 shadow-sm">
      <div class="card-body min-h-0 gap-3 p-4">
        <div class="flex shrink-0 items-center justify-between">
          <h2 class="card-title text-sm">预览</h2>
          {#if info}<span class="badge badge-ghost badge-sm">{info.cols} × 10</span>{/if}
        </div>
        <div class="relative min-h-0 flex-1 overflow-auto rounded-box border border-base-300 bg-base-200">
          {#if info}
            <BreadboardPreview {preset} cols={info.cols} />
          {:else}
            <div class="absolute inset-0 grid place-items-center"><span class="loading loading-spinner loading-md text-primary"></span></div>
          {/if}
        </div>
      </div>
    </section>
  </div>
</div>
