<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { onMount } from "svelte";
  import { nextBoardHalfSelection } from "$lib/boardHalfSelection.js";
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

  type Info = {
    preset: BreadboardPreset;
    cols: number;
    holes: number;
    has_power_rails: boolean;
    use_upper_half: boolean;
    use_lower_half: boolean;
  };
  type PowerNetOptions = {
    net_names: string[];
    positive_net: string | null;
    negative_net: string | null;
  };

  const PRESETS: { id: BreadboardPreset; name: string; defaultCols: number }[] = [
    { id: "hole170", name: ui.step2.holes(170), defaultCols: 17 },
    { id: "hole400", name: ui.step2.holes(400), defaultCols: 30 },
    { id: "hole830", name: ui.step2.holes(830), defaultCols: 63 },
  ];
  const STEP2_FIT_REFERENCE = { preset: "hole830", boardCols: 63 } as const;

  let preset = $state<BreadboardPreset>("hole400");
  let useUpperHalf = $state(true);
  let useLowerHalf = $state(true);
  let previewWidth = $state(0);
  let previewHeight = $state(0);
  let info = $state<Info | null>(null);
  let netNames = $state<string[]>([]);
  let topPositiveNet = $state("");
  let topNegativeNet = $state("");
  let bottomPositiveNet = $state("");
  let bottomNegativeNet = $state("");
  let powerOptionsReady = $state(false);
  let busy = $state(false);
  let error = $state("");
  let hasPowerRails = $derived(preset !== "hole170");
  let submitGeneration = 0;

  onMount(() => {
    void loadPowerNetOptions();
  });

  async function loadPowerNetOptions() {
    let loaded = false;
    busy = true;
    error = "";
    onStatusChange(false);
    try {
      const options = await invoke<PowerNetOptions>("get_power_net_options", { preset, locale });
      netNames = options.net_names;
      topPositiveNet = options.positive_net ?? "";
      topNegativeNet = options.negative_net ?? "";
      bottomPositiveNet = options.positive_net ?? "";
      bottomNegativeNet = options.negative_net ?? "";
      powerOptionsReady = true;
      loaded = true;
    } catch (e) {
      powerOptionsReady = false;
      error = String(e);
    } finally {
      busy = false;
    }
    if (loaded) submitNow();
  }

  function pick(p: BreadboardPreset) {
    if (busy) return;
    preset = p;
    submitNow(p);
  }

  function submitNow(p = preset) {
    if (powerOptionsReady) void submit(p);
  }

  function toggleBoardHalf(half: "upper" | "lower", enabled: boolean) {
    ({ useUpperHalf, useLowerHalf } = nextBoardHalfSelection(
      { useUpperHalf, useLowerHalf },
      half,
      enabled,
    ));
    submitNow();
  }

  async function submit(p: BreadboardPreset) {
    const generation = ++submitGeneration;
    const usePowerRails = p !== "hole170";
    busy = true;
    error = "";
    onStatusChange(false);
    try {
      const nextInfo = await invoke<Info>("set_breadboard", {
        preset: p,
        useUpperHalf,
        useLowerHalf,
        powerNets: {
          top_positive_net: usePowerRails && useUpperHalf && topPositiveNet ? topPositiveNet : null,
          top_negative_net: usePowerRails && useUpperHalf && topNegativeNet ? topNegativeNet : null,
          bottom_positive_net: usePowerRails && useLowerHalf && bottomPositiveNet ? bottomPositiveNet : null,
          bottom_negative_net: usePowerRails && useLowerHalf && bottomNegativeNet ? bottomNegativeNet : null,
        },
        locale,
      });
      if (generation !== submitGeneration) return;
      info = nextInfo;
      onBoardChange({ preset: p, boardCols: info.cols, useUpperHalf: info.use_upper_half, useLowerHalf: info.use_lower_half });
      onStatusChange(true);
    } catch (e) {
      if (generation !== submitGeneration) return;
      info = null;
      onBoardChange(null);
      error = String(e);
    } finally {
      if (generation === submitGeneration) busy = false;
    }
  }

  function observePreview(node: HTMLElement) {
    const observer = new ResizeObserver(([entry]) => {
      previewWidth = entry.contentRect.width;
      previewHeight = entry.contentRect.height;
    });
    observer.observe(node);
    return { destroy: () => observer.disconnect() };
  }
</script>

<div class="mx-auto flex h-full w-full max-w-[1920px] flex-col gap-4 overflow-hidden p-6">
  <header class="shrink-0">
    <h1 class="text-2xl font-bold">{ui.step2.title}</h1>
  </header>

  <div class="grid min-h-0 flex-1 grid-cols-[22rem_minmax(0,1fr)] gap-4">
    <aside class="card min-h-0 overflow-y-auto border border-base-300 bg-base-100 shadow-sm">
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

        <fieldset class="fieldset" disabled={busy}>
          <label class="fieldset-label cursor-pointer items-start justify-start gap-3">
            <input
              class="toggle toggle-primary toggle-sm"
              type="checkbox"
              checked={useUpperHalf}
              onchange={(event) => toggleBoardHalf("upper", event.currentTarget.checked)}
            />
            <span>{ui.step2.useUpperHalf}</span>
          </label>
          <label class="fieldset-label cursor-pointer items-start justify-start gap-3">
            <input
              class="toggle toggle-primary toggle-sm"
              type="checkbox"
              checked={useLowerHalf}
              onchange={(event) => toggleBoardHalf("lower", event.currentTarget.checked)}
            />
            <span>{ui.step2.useLowerHalf}</span>
          </label>
          <p class="label whitespace-normal text-xs text-base-content/60">{ui.step2.halfBoardHint}</p>
        </fieldset>

        <p class="text-xs leading-relaxed text-base-content/60">{ui.step2.autoBoardHint}</p>

        {#if hasPowerRails}
          <fieldset class="fieldset" disabled={busy || !powerOptionsReady}>
            <legend class="fieldset-legend">{ui.step2.powerRailBinding}</legend>
            {#if useUpperHalf}
              <p class="label mt-1 font-semibold">{ui.step2.topPowerRails}</p>
              <div class="grid grid-cols-2 gap-2">
                <label class="fieldset-label flex-col items-stretch gap-1" for="top-negative-power-net">
                  <span>{ui.step2.negativeRail}</span>
                  <select
                    id="top-negative-power-net"
                    class="select w-full min-w-0 font-mono"
                    value={topNegativeNet}
                    onchange={(event) => {
                      topNegativeNet = event.currentTarget.value;
                      submitNow();
                    }}
                  >
                    <option value="">{ui.step2.unbound}</option>
                    {#each netNames as net}<option value={net}>{net}</option>{/each}
                  </select>
                </label>
                <label class="fieldset-label flex-col items-stretch gap-1" for="top-positive-power-net">
                  <span>{ui.step2.positiveRail}</span>
                  <select
                    id="top-positive-power-net"
                    class="select w-full min-w-0 font-mono"
                    value={topPositiveNet}
                    onchange={(event) => {
                      topPositiveNet = event.currentTarget.value;
                      submitNow();
                    }}
                  >
                    <option value="">{ui.step2.unbound}</option>
                    {#each netNames as net}<option value={net}>{net}</option>{/each}
                  </select>
                </label>
              </div>
            {/if}

            {#if useLowerHalf}
              <p class="label font-semibold" class:mt-2={useUpperHalf}>{ui.step2.bottomPowerRails}</p>
              <div class="grid grid-cols-2 gap-2">
                <label class="fieldset-label flex-col items-stretch gap-1" for="bottom-negative-power-net">
                  <span>{ui.step2.negativeRail}</span>
                  <select
                    id="bottom-negative-power-net"
                    class="select w-full min-w-0 font-mono"
                    value={bottomNegativeNet}
                    onchange={(event) => {
                      bottomNegativeNet = event.currentTarget.value;
                      submitNow();
                    }}
                  >
                    <option value="">{ui.step2.unbound}</option>
                    {#each netNames as net}<option value={net}>{net}</option>{/each}
                  </select>
                </label>
                <label class="fieldset-label flex-col items-stretch gap-1" for="bottom-positive-power-net">
                  <span>{ui.step2.positiveRail}</span>
                  <select
                    id="bottom-positive-power-net"
                    class="select w-full min-w-0 font-mono"
                    value={bottomPositiveNet}
                    onchange={(event) => {
                      bottomPositiveNet = event.currentTarget.value;
                      submitNow();
                    }}
                  >
                    <option value="">{ui.step2.unbound}</option>
                    {#each netNames as net}<option value={net}>{net}</option>{/each}
                  </select>
                </label>
              </div>
            {/if}
            {#if useUpperHalf && useLowerHalf}
              <p class="label whitespace-normal text-xs text-base-content/60">{ui.step2.powerRailHint}</p>
            {/if}
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
          {#if info}<span class="badge badge-ghost badge-sm">{info.cols} × {(info.use_upper_half ? 5 : 0) + (info.use_lower_half ? 5 : 0)}</span>{/if}
        </div>
        <div inert use:observePreview class="relative min-h-0 flex-1 overflow-hidden rounded-box border border-base-300 bg-base-200">
          {#if info}
            {#key `${info.preset}:${info.cols}:${info.use_upper_half}:${info.use_lower_half}`}
              <BreadboardPreview
                preset={info.preset}
                boardCols={info.cols}
                boardCount={1}
                useUpperHalf={info.use_upper_half}
                useLowerHalf={info.use_lower_half}
                fitWidth={previewWidth}
                fitHeight={previewHeight}
                fitReference={STEP2_FIT_REFERENCE}
                panCanvas={false}
                tieNegativeRails={topNegativeNet === bottomNegativeNet}
                tiePositiveRails={topPositiveNet === bottomPositiveNet}
              />
            {/key}
          {:else}
            <div class="absolute inset-0 grid place-items-center"><span class="loading loading-spinner loading-md text-primary"></span></div>
          {/if}
        </div>
      </div>
    </section>
  </div>
</div>
