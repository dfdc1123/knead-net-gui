<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { listen, type UnlistenFn } from "@tauri-apps/api/event";
  import { onMount } from "svelte";
  import { locale, ui } from "$lib/i18n";
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
    upperHalfOnly = false,
    onComplete = () => {},
  }: {
    preset?: BreadboardPreset;
    boardCols?: number;
    upperHalfOnly?: boolean;
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
    { id: "spectral", label: "Initial" },
    { id: "annealing", label: "SA" },
    { id: "routing", label: "Routing" },
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

  let busy = $derived(phase !== "idle" && phase !== "done" && phase !== "error");
  let safeProgress = $derived(Math.max(0, Math.min(100, Number(progress) || 0)));
  let activeIndex = $derived(phases.findIndex((item) => item.id === phase));
  let selectedProfile = $derived(profiles.find((item) => item.id === profileId) ?? profiles[0]);
  let previewBoardCols = $derived(frame?.board_cols ?? boardCols);
  let previewBoardCount = $derived(frame?.board_count ?? 1);

  function stepClass(index: number) {
    if (phase === "done" || index <= activeIndex) return "step step-primary";
    return "step";
  }

  function applyEvent(event: ComputeProgressEvent) {
    if (activeRunId === null) activeRunId = event.run_id;
    if (event.run_id !== activeRunId) return;

    phase = event.phase;
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
    if (event.frame) frame = event.frame;
    if (event.phase === "error") error = event.message;
    if (event.phase === "routing" || event.phase === "done" || event.phase === "error") {
      interrupting = false;
    }
    if (event.phase === "done" && frame) onComplete(frame);
  }

  function enqueue(event: ComputeProgressEvent) {
    if (!busy || (activeRunId !== null && event.run_id !== activeRunId)) return;
    if (interrupting && event.phase === "annealing" && event.progress < 88) return;
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
    };
  });

  async function start() {
    if (busy || !listenerReady) return;

    queue = [];
    activeRunId = null;
    frame = null;
    error = "";
    interrupting = false;
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

<div class="mx-auto flex h-full w-full max-w-[1920px] flex-col gap-4 overflow-hidden p-6">
  <header class="flex shrink-0 items-center justify-between gap-3">
    <h1 class="text-2xl font-bold">{ui.step3.title}</h1>
    {#if phase === "annealing"}
      <button class="btn btn-sm btn-warning" onclick={interruptAndRoute} disabled={interrupting}>
        {#if interrupting}<span class="loading loading-spinner loading-xs"></span>{/if}
        {interrupting ? ui.step3.interrupting : ui.step3.interruptAndRoute}
      </button>
    {:else}
      <button class="btn btn-sm btn-primary" onclick={start} disabled={busy || !listenerReady}>
        {#if busy}<span class="loading loading-spinner loading-xs"></span>{/if}
        {phase === "done" || phase === "error" ? ui.step3.recompute : busy ? ui.step3.computing : ui.step3.start}
      </button>
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
            <span class="flex min-w-0 items-center gap-2 truncate font-medium"><span class="status {busy ? 'status-info' : phase === 'error' ? 'status-error' : phase === 'done' ? 'status-success' : 'status-neutral'}"></span>{message}</span>
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
          <h2 class="card-title text-sm">{ui.common.preview}</h2>
          <div class="flex gap-2">
            {#if frame?.iteration !== undefined}<span class="badge badge-ghost badge-sm">#{frame.iteration}</span>{/if}
            {#if frame?.cost !== undefined}<span class="badge badge-secondary badge-sm">{frame.cost.toFixed(2)}</span>{/if}
          </div>
        </div>

        <div class="relative min-h-0 flex-1 overflow-hidden rounded-box border border-base-300 bg-base-200">
          <div inert class="h-full overflow-hidden">
            <BreadboardPreview
              {preset}
              boardCols={previewBoardCols}
              boardCount={previewBoardCount}
              {frame}
              {upperHalfOnly}
              panCanvas={false}
              solidWires={phase === "done"}
            />
          </div>
          {#if !frame}
            <div class="pointer-events-none absolute inset-0 z-10 grid place-items-center bg-base-200/75">
              {#if busy}<span class="loading loading-spinner loading-lg text-primary"></span>{:else}<span class="text-sm text-base-content/50">{ui.step3.waiting}</span>{/if}
            </div>
          {/if}
        </div>
      </div>
    </section>
  </div>
</div>
