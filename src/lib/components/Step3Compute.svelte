<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { listen, type UnlistenFn } from "@tauri-apps/api/event";
  import { onMount } from "svelte";
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
    cols = 30,
    onComplete = () => {},
  }: {
    preset?: BreadboardPreset;
    cols?: number;
    onComplete?: (frame: LayoutFrame) => void;
  } = $props();

  type ProfileId = "quick" | "standard" | "full";
  type ComputeProfile = { id: ProfileId; name: string; description: string };

  const profiles: ComputeProfile[] = [
    { id: "quick", name: "快速", description: "8 seeds · 5,000 次" },
    { id: "standard", name: "标准", description: "32 seeds · 200,000 次" },
    { id: "full", name: "完整", description: "100 seeds · 1,000,000 次" },
  ];
  const phases: { id: Exclude<ComputePhase, "idle" | "error">; label: string; hint: string }[] = [
    { id: "spectral", label: "Spectral", hint: "生成初始布局" },
    { id: "annealing", label: "SA", hint: "退火优化中" },
    { id: "routing", label: "Routing", hint: "生成跳线" },
    { id: "done", label: "完成", hint: "布局已就绪" },
  ];

  let profileId = $state<ProfileId>(import.meta.env.DEV ? "quick" : "standard");
  let phase = $state<ComputePhase>("idle");
  let progress = $state(0);
  let frame = $state<LayoutFrame | null>(null);
  let message = $state("准备计算");
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

  function stepClass(index: number) {
    if (phase === "done" || index <= activeIndex) return "step step-primary";
    return "step";
  }

  function applyEvent(event: ComputeProgressEvent) {
    if (activeRunId === null) activeRunId = event.run_id;
    if (event.run_id !== activeRunId) return;

    phase = event.phase;
    progress = event.progress;
    message = event.message;
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
        error = `无法监听计算进度：${String(reason)}`;
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
    message = `正在启动${selectedProfile.name}计算…`;

    const request: ComputeRequest = {
      profile: selectedProfile.id,
    };
    try {
      await invoke("start_compute", { request });
    } catch (reason) {
      queue = [];
      phase = "error";
      error = String(reason);
      message = "计算失败";
    }
  }

  async function interruptAndRoute() {
    if (phase !== "annealing" || interrupting) return;
    interrupting = true;
    message = "正在中断 SA，并准备为当前最佳布局布线…";
    // 丢掉尚未播放的旧 SA 帧；保留可能已经抵达的选优/routing/final 事件。
    queue = queue.filter((event) => event.phase !== "annealing" || event.progress >= 88);
    try {
      const accepted = await invoke<boolean>("cancel_compute");
      if (!accepted) {
        message = "SA 已经结束，正在切换到布线结果…";
      }
    } catch (reason) {
      interrupting = false;
      error = `无法中断计算：${String(reason)}`;
    }
  }
</script>

<div class="flex h-full flex-col gap-4 overflow-auto p-6">
  <div class="flex flex-wrap items-start justify-between gap-3">
    <div>
      <h2 class="text-xs font-semibold uppercase tracking-wider text-base-content/50">计算与布局过程</h2>
      <p class="mt-1 text-sm text-base-content/60">初始布局、退火优化和布线会在同一块面包板上连续显示。</p>
    </div>
    {#if phase === "annealing"}
      <button class="btn btn-sm btn-warning" onclick={interruptAndRoute} disabled={interrupting}>
        {#if interrupting}<span class="loading loading-spinner loading-xs"></span>{/if}
        {interrupting ? "正在中断" : "中断并布线"}
      </button>
    {:else}
      <button class="btn btn-sm btn-primary" onclick={start} disabled={busy || !listenerReady}>
        {#if busy}<span class="loading loading-spinner loading-xs"></span>{/if}
        {phase === "done" || phase === "error" ? "重新计算" : busy ? "计算中" : "开始计算"}
      </button>
    {/if}
  </div>

  <div class="card border border-base-300 bg-base-200 shadow-sm">
    <div class="card-body gap-4 p-4">
      <fieldset disabled={busy} class="grid grid-cols-3 gap-2">
        <legend class="sr-only">计算强度</legend>
        {#each profiles as item}
          <label class="btn h-auto min-h-12 px-3 py-2 {profileId === item.id ? 'btn-primary' : 'btn-ghost bg-base-100'}">
            <input class="sr-only" type="radio" name="compute-profile" value={item.id} bind:group={profileId} />
            <span class="min-w-0 text-left">
              <span class="block text-sm font-semibold">{item.name}</span>
              <span class="block truncate text-[10px] font-normal opacity-65">{item.description}</span>
            </span>
          </label>
        {/each}
      </fieldset>

      <ul class="steps steps-horizontal w-full text-xs">
        {#each phases as item, index}
          <li class={stepClass(index)} data-content={phase === item.id && busy ? "●" : undefined}>
            <span class="hidden sm:inline">{item.label}</span>
          </li>
        {/each}
      </ul>
      <div class="flex items-center justify-between gap-3 text-xs">
        <span class="font-medium">{message}</span>
        <span class="tabular-nums text-base-content/50">{Math.round(safeProgress)}%</span>
      </div>
      <progress class="progress progress-primary h-2 w-full" value={safeProgress} max="100"></progress>
    </div>
  </div>

  {#if error}
    <div class="alert alert-error text-sm" role="alert"><span>{error}</span></div>
  {/if}

  <div class="card min-h-0 flex-1 border border-base-300 bg-base-200 shadow-sm">
    <div class="card-body min-h-0 gap-3 p-4">
      <div class="flex flex-wrap items-center justify-between gap-2">
        <div>
          <h3 class="card-title text-sm">布局预览</h3>
          <p class="text-xs text-base-content/50">
            {phase === "annealing" ? "正在展示固定观察种子的优化轨迹；最终结果仍取所有种子中的最低成本布局。" : phases.find((item) => item.id === phase)?.hint ?? "等待开始"}
          </p>
        </div>
        <div class="flex gap-2">
          {#if frame?.iteration !== undefined}<span class="badge badge-ghost badge-sm">迭代 {frame.iteration}</span>{/if}
          {#if frame?.cost !== undefined}<span class="badge badge-secondary badge-sm">Cost {frame.cost.toFixed(2)}</span>{/if}
        </div>
      </div>

      <div class="relative min-h-72 flex-1 overflow-auto rounded-box bg-base-100">
        <BreadboardPreview {preset} {cols} {frame} />
        {#if !frame}
          <div class="pointer-events-none absolute inset-0 grid place-items-center bg-base-100/65">
            <div class="text-center text-base-content/45">
              {#if busy}
                <span class="loading loading-spinner loading-lg text-primary"></span>
                <p class="mt-2 text-sm">等待第一个布局快照…</p>
              {:else}
                <p class="text-sm">点击“开始计算”查看布局过程</p>
              {/if}
            </div>
          </div>
        {/if}
      </div>
    </div>
  </div>
</div>
