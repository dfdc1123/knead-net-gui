<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";

  type Preset = "hole170" | "hole400" | "hole800";
  type Info = { preset: string; cols: number; holes: number; has_power_rails: boolean };

  const PRESETS: { id: Preset; name: string; desc: string; defaultCols: number; rail: boolean }[] = [
    { id: "hole170", name: "170 孔", desc: "迷你 17×10 main", defaultCols: 17, rail: false },
    { id: "hole400", name: "400 孔", desc: "标准 30×10 main + 电源轨", defaultCols: 30, rail: true },
    { id: "hole800", name: "800 孔", desc: "加宽 63×10 main + 宽电源轨", defaultCols: 63, rail: true },
  ];

  let preset = $state<Preset>("hole400");
  let cols = $state(30);
  let info = $state<Info | null>(null);
  let busy = $state(false);

  function pick(p: Preset) {
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

  async function submit(p: Preset, c: number) {
    busy = true;
    try {
      info = await invoke<Info>("set_breadboard", { preset: p, cols: c });
    } catch (e) {
      console.error(e);
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

  {#if info}
    <div class="card bg-base-200 p-4">
      <h3 class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-3">
        预览 · {info.cols} × 12
      </h3>
      <div class="bg-white rounded p-2 overflow-auto">
        <div class="flex flex-col gap-1.5 select-none" style="width: max-content">
          {#if info.has_power_rails}
            {#each [0, 1] as _railIdx}
              <div class="grid gap-px" style="grid-template-columns: repeat({info.cols}, 6px)">
                {#each Array(info.cols) as _, c}
                  <div class="w-1.5 h-1.5 rounded-full {c % 6 < 5 ? 'bg-red-400' : 'bg-base-300'}"></div>
                {/each}
              </div>
            {/each}
          {/if}
          {#each Array(12) as _, r}
            <div class="grid gap-px" style="grid-template-columns: repeat({info.cols}, 6px)">
              {#each Array(info.cols) as _, c}
                <div class="w-1.5 h-1.5 rounded-full
                            {r === 5 || r === 6 ? 'bg-base-300' : 'bg-base-content/40'}"></div>
              {/each}
            </div>
          {/each}
          {#if info.has_power_rails}
            {#each [0, 1] as _railIdx}
              <div class="grid gap-px" style="grid-template-columns: repeat({info.cols}, 6px)">
                {#each Array(info.cols) as _, c}
                  <div class="w-1.5 h-1.5 rounded-full {c % 6 < 5 ? 'bg-blue-400' : 'bg-base-300'}"></div>
                {/each}
              </div>
            {/each}
          {/if}
        </div>
      </div>
    </div>
  {/if}
</div>