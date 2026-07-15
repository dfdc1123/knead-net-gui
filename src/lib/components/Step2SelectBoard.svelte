<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { onMount } from "svelte";
  import { locale, ui } from "$lib/i18n";
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
  type PowerNetOptions = {
    net_names: string[];
    positive_net: string | null;
    negative_net: string | null;
  };

  const PRESETS: { id: BreadboardPreset; name: string; defaultCols: number }[] = [
    { id: "hole170", name: ui.step2.holes(170), defaultCols: 17 },
    { id: "hole400", name: ui.step2.holes(400), defaultCols: 30 },
    { id: "hole800", name: ui.step2.holes(800), defaultCols: 63 },
  ];

  let preset = $state<BreadboardPreset>("hole400");
  let cols = $state(30);
  let info = $state<Info | null>(null);
  let netNames = $state<string[]>([]);
  let positiveNet = $state("");
  let negativeNet = $state("");
  let powerOptionsReady = $state(false);
  let busy = $state(false);
  let error = $state("");
  let hasPowerRails = $derived(preset !== "hole170");

  onMount(() => {
    void loadPowerNetOptions();
  });

  async function loadPowerNetOptions() {
    busy = true;
    error = "";
    onStatusChange(false);
    try {
      const options = await invoke<PowerNetOptions>("get_power_net_options", { preset, locale });
      netNames = options.net_names;
      positiveNet = options.positive_net ?? "";
      negativeNet = options.negative_net ?? "";
      powerOptionsReady = true;
    } catch (e) {
      powerOptionsReady = false;
      error = String(e);
    } finally {
      busy = false;
    }
  }

  function pick(p: BreadboardPreset) {
    if (busy) return;
    preset = p;
    cols = PRESETS.find((x) => x.id === p)!.defaultCols;
  }

  // cols 变化 → 自动重提交 (debounce 250ms)
  let timer: ReturnType<typeof setTimeout> | null = null;
  $effect(() => {
    cols; preset; positiveNet; negativeNet; powerOptionsReady;
    if (timer) clearTimeout(timer);
    if (!powerOptionsReady) return;
    timer = setTimeout(() => submit(preset, cols), 250);
  });

  async function submit(p: BreadboardPreset, c: number) {
    busy = true;
    error = "";
    onStatusChange(false);
    try {
      info = await invoke<Info>("set_breadboard", {
        preset: p,
        cols: c,
        positiveNet: hasPowerRails && positiveNet ? positiveNet : null,
        negativeNet: hasPowerRails && negativeNet ? negativeNet : null,
        locale,
      });
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

<div class="mx-auto flex h-full w-full max-w-[1920px] flex-col gap-4 overflow-hidden p-6">
  <header class="shrink-0">
    <h1 class="text-2xl font-bold">{ui.step2.title}</h1>
  </header>

  <div class="grid min-h-0 flex-1 grid-cols-[22rem_minmax(0,1fr)] gap-4">
    <aside class="card min-h-0 border border-base-300 bg-base-100 shadow-sm">
      <div class="card-body gap-4 p-4">
        <fieldset class="fieldset" disabled={busy}>
          <legend class="fieldset-legend">{ui.step2.boardType}</legend>
          <div class="join join-vertical w-full">
            {#each PRESETS as p}
              <label class="join-item flex cursor-pointer items-center gap-3 border border-base-300 px-4 py-3 hover:bg-base-200" class:bg-base-200={preset === p.id}>
                <input
                  type="radio"
                  class="radio radio-primary radio-sm"
                  name="breadboard-preset"
                  checked={preset === p.id}
                  onchange={() => pick(p.id)}
                  aria-label={ui.step2.selectPreset(p.name)}
                />
                <span class="flex-1 font-semibold">{p.name}</span>
                <span class="badge badge-ghost badge-sm">{ui.step2.columns(p.defaultCols)}</span>
              </label>
            {/each}
          </div>
        </fieldset>

        <fieldset class="fieldset">
          <legend class="fieldset-legend">{ui.step2.columnCount}</legend>
          <label class="input w-full">
            <input type="number" min="3" max="120" bind:value={cols} aria-label={ui.step2.availableColumns} />
            <span class="label">3–120</span>
          </label>
        </fieldset>

        {#if hasPowerRails}
          <fieldset class="fieldset" disabled={busy || !powerOptionsReady}>
            <legend class="fieldset-legend">{ui.step2.powerRailBinding}</legend>
            <label class="fieldset-label" for="positive-power-net">{ui.step2.positiveRail}</label>
            <select id="positive-power-net" class="select w-full font-mono" bind:value={positiveNet}>
              <option value="">{ui.step2.unbound}</option>
              {#each netNames as net}
                <option value={net}>{net}</option>
              {/each}
            </select>

            <label class="fieldset-label mt-2" for="negative-power-net">{ui.step2.negativeRail}</label>
            <select id="negative-power-net" class="select w-full font-mono" bind:value={negativeNet}>
              <option value="">{ui.step2.unbound}</option>
              {#each netNames as net}
                <option value={net}>{net}</option>
              {/each}
            </select>
            <p class="label whitespace-normal text-xs text-base-content/60">{ui.step2.powerRailHint}</p>
          </fieldset>
        {/if}

        {#if info}
          <div class="flex flex-wrap gap-2">
            <span class="badge badge-primary">{ui.step2.holes(info.holes)}</span>
            <span class="badge badge-outline">{info.has_power_rails ? ui.step2.withRails : ui.step2.withoutRails}</span>
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
          <h2 class="card-title text-sm">{ui.common.preview}</h2>
          {#if info}<span class="badge badge-ghost badge-sm">{info.cols} × 10</span>{/if}
        </div>
        <div inert class="relative min-h-0 flex-1 overflow-hidden rounded-box border border-base-300 bg-base-200">
          {#if info}
            <BreadboardPreview {preset} cols={info.cols} panCanvas={false} />
          {:else}
            <div class="absolute inset-0 grid place-items-center"><span class="loading loading-spinner loading-md text-primary"></span></div>
          {/if}
        </div>
      </div>
    </section>
  </div>
</div>
