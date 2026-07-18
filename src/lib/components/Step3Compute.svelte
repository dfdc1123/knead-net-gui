<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { listen, type UnlistenFn } from "@tauri-apps/api/event";
  import { onMount } from "svelte";
  import { locale, ui } from "$lib/i18n";
  import { isBetterSeedCost } from "$lib/seedPreview.js";
  import type {
    BreadboardPreset,
    ComputePhase,
    ComputeProgressEvent,
    ComputeRequest,
    LayoutFrame,
  } from "$lib/layout";
  import BreadboardPreview from "./BreadboardPreview.svelte";

  let {
    preset = "hole400",
    boardCols = 30,
    useUpperHalf = true,
    useLowerHalf = true,
    onComplete = () => {},
  }: {
    preset?: BreadboardPreset;
    boardCols?: number;
    useUpperHalf?: boolean;
    useLowerHalf?: boolean;
    onComplete?: (frame: LayoutFrame) => void;
  } = $props();

  type ProfileId = "quick" | "standard" | "full";
  type ComputeProfile = { id: ProfileId; name: string; description: string };

  const profiles: ComputeProfile[] = [
    { id: "quick", name: ui.step3.profiles.quick, description: ui.step3.profileDescriptions.quick },
    { id: "standard", name: ui.step3.profiles.standard, description: ui.step3.profileDescriptions.standard },
    { id: "full", name: ui.step3.profiles.full, description: ui.step3.profileDescriptions.full },
  ];
  const phases: { id: Exclude<ComputePhase, "idle" | "error">; label: string }[] = [
    { id: "spectral", label: ui.step3.phaseInitial },
    { id: "annealing", label: ui.step3.phaseLayout },
    { id: "routing", label: ui.step3.phaseRouting },
    { id: "done", label: ui.step3.done },
  ];

  let profileId = $state<ProfileId>(import.meta.env.DEV ? "quick" : "standard");
  let phase = $state<ComputePhase>("idle");
  let progress = $state(0);
  let frame = $state<LayoutFrame | null>(null);
  let message = $state<string>(ui.step3.ready);
  let error = $state("");
  let listenerReady = $state(false);
  let interrupting = $state(false);
  let activeRunId: string | number | null = null;
  let queue: ComputeProgressEvent[] = [];
  let playbackTimer: ReturnType<typeof setInterval> | undefined;
  let improvementTimer: ReturnType<typeof setTimeout> | undefined;
  let previewMode = $state<"observing" | "best">("observing");
  let observedSeed = $state<number | null>(null);
  let completedSeeds = $state(0);
  let totalSeeds = $state(0);
  let bestSeed = $state<number | null>(null);
  let bestCost = $state<number | null>(null);
  let bestFrame = $state<LayoutFrame | null>(null);
  let improvementMessage = $state("");

  let busy = $derived(phase !== "idle" && phase !== "done" && phase !== "error");
  let safeProgress = $derived(Math.max(0, Math.min(100, Number(progress) || 0)));
  let activeIndex = $derived(phases.findIndex((item) => item.id === phase));
  let selectedProfile = $derived(profiles.find((item) => item.id === profileId) ?? profiles[0]);
  let previewBoardCols = $derived(frame?.board_cols ?? boardCols);
  let previewBoardCount = $derived(frame?.board_count ?? 1);
  let remainingSeeds = $derived(Math.max(0, totalSeeds - completedSeeds));
  let showingBestSearch = $derived(
    phase === "annealing" && previewMode === "best" && remainingSeeds > 0 && progress < 88,
  );

  function stepClass(index: number) {
    if (phase === "done" || index <= activeIndex) return "step step-primary";
    return "step";
  }

  function isObservedPreviewFrame(event: ComputeProgressEvent) {
    return (
      !event.seed_result &&
      Boolean(event.frame) &&
      event.progress <= 10 &&
      (event.phase === "spectral" || event.phase === "annealing")
    );
  }

  function resetSeedPreview() {
    previewMode = "observing";
    observedSeed = null;
    completedSeeds = 0;
    totalSeeds = 0;
    bestSeed = null;
    bestCost = null;
    bestFrame = null;
    improvementMessage = "";
    if (improvementTimer) clearTimeout(improvementTimer);
  }

  function showImprovement(previous: number, next: number) {
    improvementMessage = ui.step3.betterLayout(previous, next);
    if (improvementTimer) clearTimeout(improvementTimer);
    improvementTimer = setTimeout(() => {
      improvementMessage = "";
    }, 1800);
  }

  function applySeedResult(event: ComputeProgressEvent) {
    const result = event.seed_result;
    if (!result) return;

    if (event.progress >= progress) message = event.message;
    progress = Math.max(progress, event.progress);
    completedSeeds = Math.max(completedSeeds, result.completed);
    totalSeeds = Math.max(totalSeeds, result.total);

    const previousBest = bestCost;
    if (event.frame && isBetterSeedCost(bestCost, result.cost)) {
      bestSeed = result.seed;
      bestCost = result.cost;
      bestFrame = event.frame;
      if (previewMode === "best") {
        frame = event.frame;
        if (previousBest !== null) showImprovement(previousBest, result.cost);
      }
    }

    if (result.observed) {
      // The fixed observed seed is done. Discard its delayed animation and show the
      // best completed candidate while the remaining workers continue searching.
      queue = queue.filter((queued) => !queued.frame || queued.progress >= 88);
      previewMode = "best";
      if (bestFrame) frame = bestFrame;
    }
  }

  function applyEvent(event: ComputeProgressEvent) {
    if (activeRunId === null) activeRunId = event.run_id;
    if (event.run_id !== activeRunId) return;

    if (event.phase === "spectral" && event.progress === 0 && !event.frame) {
      // A larger-board retry starts a fresh set of seeds; costs from different
      // board geometries must never compete for the same best preview.
      queue = [];
      frame = null;
      resetSeedPreview();
    }
    if (previewMode === "best" && isObservedPreviewFrame(event)) return;
    phase = event.phase;
    if (event.seed_result) {
      applySeedResult(event);
      return;
    }
    const animationFrame = event.phase === "annealing" && Boolean(event.frame) && event.progress <= 10;
    const aggregateEvent = event.phase === "annealing" && !event.frame && event.progress < 88;
    if (animationFrame) {
      // 动画帧只描述观察 seed，不得覆盖全部 seeds 的真实完成进度。
      progress = Math.max(progress, event.progress);
      if (progress <= 10) message = event.message;
    } else if (!aggregateEvent || event.progress >= progress) {
      // 并行 worker 的完成事件可能极短暂地乱序；总进度只允许前进。
      progress = event.progress;
      message = event.message;
    }
    if ((event.phase === "spectral" || animationFrame) && event.seed !== undefined) {
      observedSeed = event.seed;
    }
    if (event.frame && (!isObservedPreviewFrame(event) || previewMode === "observing")) {
      frame = event.frame;
    }
    if (event.phase === "annealing" && event.progress >= 88) {
      previewMode = "best";
      if (event.seed !== undefined) bestSeed = event.seed;
      if (event.frame?.cost !== undefined) bestCost = event.frame.cost;
      if (event.frame) bestFrame = event.frame;
    }
    if (event.phase === "error") error = event.message;
    if (event.phase === "routing" || event.phase === "done" || event.phase === "error") {
      interrupting = false;
    }
    if (event.phase === "done" && frame) onComplete(frame);
  }

  function enqueue(event: ComputeProgressEvent) {
    if (!busy || (activeRunId !== null && event.run_id !== activeRunId)) return;
    if (interrupting && event.phase === "annealing" && event.progress < 88) return;
    if (previewMode === "best" && isObservedPreviewFrame(event)) return;
    if (event.seed_result) {
      // Candidate completions drive the real aggregate progress and best preview;
      // they must not wait behind the decorative 80 ms animation queue.
      applyEvent(event);
      return;
    }
    if (!event.frame) {
      // seeds 聚合进度不参与 80ms 动画排队，否则多个完成事件会产生额外延迟。
      applyEvent(event);
      return;
    }
    if (event.progress >= 88 || event.phase === "routing" || event.phase === "done") {
      // placement/routing 已经抵达时，旧的观察 seed 动画不应把结果再拖十秒。
      queue = queue.filter((queued) => queued.progress >= 88);
    }
    queue.push(event);
  }

  onMount(() => {
    let disposed = false;
    let unlisten: UnlistenFn | undefined;

    void listen<ComputeProgressEvent>("compute-progress", ({ payload }) => enqueue(payload)).then(
      (stop) => {
        if (disposed) stop();
        else {
          unlisten = stop;
          listenerReady = true;
        }
      },
      (reason) => {
        error = ui.step3.listenerError(String(reason));
        phase = "error";
      },
    );

    playbackTimer = setInterval(() => {
      const next = queue.shift();
      if (next) applyEvent(next);
    }, 80);

    return () => {
      disposed = true;
      unlisten?.();
      if (playbackTimer) clearInterval(playbackTimer);
      if (improvementTimer) clearTimeout(improvementTimer);
    };
  });

  async function start() {
    if (busy || !listenerReady) return;

    queue = [];
    activeRunId = null;
    frame = null;
    error = "";
    interrupting = false;
    resetSeedPreview();
    progress = 0;
    phase = "spectral";
    message = ui.step3.starting(selectedProfile.name);

    const request: ComputeRequest = {
      profile: selectedProfile.id,
      locale,
    };
    try {
      await invoke("start_compute", { request });
    } catch (reason) {
      queue = [];
      phase = "error";
      error = String(reason);
      message = ui.step3.computeFailed;
    }
  }

  async function interruptAndRoute() {
    if (phase !== "annealing" || interrupting) return;
    interrupting = true;
    message = ui.step3.interruptingRoute;
    // 丢掉尚未播放的旧 SA 帧；保留可能已经抵达的选优/routing/final 事件。
    queue = queue.filter((event) => event.phase !== "annealing" || event.progress >= 88);
    try {
      const accepted = await invoke<boolean>("cancel_compute");
      if (!accepted) {
        message = ui.step3.saAlreadyDone;
      }
    } catch (reason) {
      interrupting = false;
      error = ui.step3.interruptError(String(reason));
    }
  }
</script>

<div class="mx-auto flex h-full min-h-0 w-full max-w-[1920px] flex-col gap-4 overflow-hidden p-6">
  <header class="flex shrink-0 items-center justify-between gap-3">
    <h1 class="text-2xl font-bold">{ui.step3.title}</h1>
    {#if phase === "annealing"}
      <button class="btn btn-sm btn-warning" onclick={interruptAndRoute} disabled={interrupting}>
        {#if interrupting}<span class="loading loading-spinner loading-xs"></span>{/if}
        {interrupting ? ui.step3.interrupting : ui.step3.interruptAndRoute}
      </button>
    {:else}
      {#if phase === "idle" && listenerReady}
        <span class="aura aura-sm workflow-next-step text-primary">
          <button class="btn btn-sm btn-primary" onclick={start}>
            {ui.step3.start}
          </button>
        </span>
      {:else}
        <button class="btn btn-sm btn-primary" onclick={start} disabled={busy || !listenerReady}>
          {#if busy}<span class="loading loading-spinner loading-xs"></span>{/if}
          {phase === "done" || phase === "error" ? ui.step3.recompute : busy ? ui.step3.computing : ui.step3.start}
        </button>
      {/if}
    {/if}
  </header>

  <div class="grid min-h-0 flex-1 grid-cols-[23rem_minmax(0,1fr)] gap-4">
    <aside class="card min-h-0 overflow-y-auto border border-base-300 bg-base-100 shadow-sm">
      <div class="card-body min-h-0 gap-4 p-4">
        <fieldset class="fieldset shrink-0" disabled={busy}>
          <legend class="fieldset-legend">{ui.step3.strength}</legend>
          <div class="join join-vertical w-full">
            {#each profiles as item}
              <label class="join-item flex cursor-pointer items-center gap-3 border border-base-300 px-4 py-3 hover:bg-base-200" class:bg-base-200={profileId === item.id}>
                <input class="radio radio-primary radio-sm" type="radio" name="compute-profile" value={item.id} bind:group={profileId} />
                <span class="flex-1 font-semibold">{item.name}</span>
                <span class="text-xs text-base-content/60">{item.description}</span>
              </label>
            {/each}
          </div>
        </fieldset>

        <ul class="steps steps-vertical text-sm" aria-label={ui.step3.phases}>
          {#each phases as item, index}
            <li class={stepClass(index)} data-content={phase === item.id && busy ? "●" : undefined}>{item.label}</li>
          {/each}
        </ul>

        <div class="mt-auto space-y-2">
          <div class="flex items-center justify-between gap-3 text-sm">
            <span class="flex min-w-0 items-center gap-2 truncate font-medium">
              <span
                class="status {busy ? 'status-info animate-bounce motion-reduce:animate-none' : phase === 'error' ? 'status-error' : phase === 'done' ? 'status-success' : 'status-neutral'}"
                aria-hidden="true"
              ></span>
              {message}
            </span>
            <span class="tabular-nums text-base-content/50">{Math.round(safeProgress)}%</span>
          </div>
          <progress class="progress progress-primary w-full" value={safeProgress} max="100"></progress>
        </div>

        {#if error}
          <div class="alert alert-error text-sm" role="alert"><span>{error}</span></div>
        {/if}
      </div>
    </aside>

    <section class="card min-h-0 border border-base-300 bg-base-100 shadow-sm">
      <div class="card-body min-h-0 gap-3 p-4">
        <div class="flex items-center justify-between gap-2">
          <div class="flex min-w-0 items-center gap-2">
            <h2 class="card-title text-sm">
              {phase === "annealing" && previewMode === "best" ? ui.step3.currentBest : ui.common.preview}
            </h2>
            {#if phase === "annealing"}
              <span class="badge badge-outline badge-sm">
                {previewMode === "best" ? ui.step3.bestSeed(bestSeed) : ui.step3.observingSeed(observedSeed)}
              </span>
            {/if}
          </div>
          <div class="flex gap-2">
            {#if previewMode === "observing" && frame?.iteration !== undefined}<span class="badge badge-ghost badge-sm">#{frame.iteration}</span>{/if}
            {#if frame?.cost !== undefined}<span class="badge badge-secondary badge-sm">{frame.cost.toFixed(2)}</span>{/if}
          </div>
        </div>

        <div class="relative min-h-0 flex-1 overflow-hidden rounded-box border border-base-300 bg-base-200">
          <div inert class="h-full overflow-hidden">
            <BreadboardPreview
              {preset}
              boardCols={previewBoardCols}
              boardCount={previewBoardCount}
              gapCols={frame?.gap_cols}
              {frame}
              {useUpperHalf}
              {useLowerHalf}
              panCanvas={false}
              solidWires={phase === "done"}
            />
          </div>
          {#if !frame}
            <div class="pointer-events-none absolute inset-0 z-10 grid place-items-center bg-base-200/75">
              {#if busy}<span class="loading loading-spinner loading-lg text-primary"></span>{:else}<span class="text-sm text-base-content/50">{ui.step3.waiting}</span>{/if}
            </div>
          {/if}
          {#if improvementMessage}
            <div class="pointer-events-none absolute left-1/2 top-3 z-20 -translate-x-1/2">
              <span class="badge badge-success whitespace-nowrap shadow-sm">{improvementMessage}</span>
            </div>
          {/if}
          {#if showingBestSearch}
            <div class="pointer-events-none absolute bottom-3 right-3 z-20 flex items-center gap-2 rounded-box border border-base-300 bg-base-100/90 px-3 py-2 text-xs font-medium shadow-sm backdrop-blur-sm">
              <span class="loading loading-dots loading-sm text-primary" aria-hidden="true"></span>
              <span>{ui.step3.remainingSeeds(remainingSeeds)}</span>
            </div>
          {/if}
        </div>
      </div>
    </section>
  </div>
</div>
