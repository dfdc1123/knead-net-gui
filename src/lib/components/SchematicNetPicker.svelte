<script lang="ts">
  import { onDestroy, tick } from "svelte";
  import { centerCanvas, centerCanvasNow } from "$lib/actions/centerCanvas";
  import { ui } from "$lib/i18n";
  import {
    clampDiagramZoom,
    createPointerPanController,
    createWheelGestureClassifier,
    createWheelZoomController,
  } from "$lib/wheelGestures.js";
  import ZoomControls from "./ZoomControls.svelte";

  let {
    open = $bindable(false),
    schematicSvg,
    selectedNet = "",
    allowedNetNames = [],
    title,
    onSelect = () => {},
  }: {
    open?: boolean;
    schematicSvg: string;
    selectedNet?: string;
    allowedNetNames?: string[];
    title: string;
    onSelect?: (net: string) => void;
  } = $props();

  let dialog = $state<HTMLDialogElement>();
  let schematicHost = $state<HTMLDivElement>();
  let zoom = $state(1);
  let viewportWidth = $state(0);
  let viewportHeight = $state(0);
  let unavailableNet = $state("");

  const classifyWheel = createWheelGestureClassifier();
  const pointerPan = createPointerPanController();
  const wheelZoom = createWheelZoomController({
    getZoom: () => zoom,
    setZoom: (nextZoom: number) => (zoom = nextZoom),
    afterRender: tick,
  });

  function clampZoom(nextZoom: number) {
    return clampDiagramZoom(nextZoom);
  }

  function syncViewportSize() {
    if (!schematicHost) return;
    const bounds = schematicHost.getBoundingClientRect();
    viewportWidth = Math.round(bounds.width);
    viewportHeight = Math.round(bounds.height);
  }

  function observeViewport(viewport: HTMLDivElement) {
    const observer = new ResizeObserver(() => {
      if (zoom !== 1) return;
      syncViewportSize();
      requestAnimationFrame(() => centerCanvasNow(viewport));
    });
    observer.observe(viewport);
    syncViewportSize();
    return { destroy: () => observer.disconnect() };
  }

  async function setZoom(nextZoom: number) {
    zoom = clampZoom(nextZoom);
    await tick();
  }

  async function resetDiagram() {
    zoom = 1;
    syncViewportSize();
    await tick();
    if (schematicHost) centerCanvasNow(schematicHost);
  }

  function handleZoomWheel(event: WheelEvent) {
    const viewport = event.currentTarget as HTMLDivElement;
    const gesture = classifyWheel(event);
    if (gesture === "pan") return;
    event.preventDefault();

    const diagram = viewport.querySelector("svg");
    if (!diagram) return;
    wheelZoom.queue(event, gesture, viewport, diagram);
  }

  function startPan(event: PointerEvent) {
    if (event.button !== 2) return;
    event.preventDefault();
    const viewport = event.currentTarget as HTMLDivElement;
    pointerPan.start(viewport, event.pointerId, event.clientX, event.clientY);
    viewport.setPointerCapture(event.pointerId);
    viewport.classList.add("is-panning");
  }

  function movePan(event: PointerEvent) {
    if (!pointerPan.isActive(event.pointerId)) return;
    event.preventDefault();
    pointerPan.move(event.pointerId, event.clientX, event.clientY);
  }

  function stopPan(event: PointerEvent) {
    if (!pointerPan.isActive(event.pointerId)) return;
    const viewport = event.currentTarget as HTMLDivElement;
    pointerPan.stop(event.pointerId);
    viewport.classList.remove("is-panning");
    if (viewport.hasPointerCapture(event.pointerId)) viewport.releasePointerCapture(event.pointerId);
  }

  function syncHighlight() {
    if (!schematicHost) return;
    for (const element of schematicHost.querySelectorAll<SVGElement>("[data-net]")) {
      const active = Boolean(selectedNet) && element.dataset.net === selectedNet;
      element.classList.toggle("is-selected", active);
      element.classList.toggle("is-muted", Boolean(selectedNet) && !active);
    }
  }

  function closeDialog() {
    open = false;
    if (dialog?.open) dialog.close();
  }

  function chooseNetwork(net: string) {
    unavailableNet = "";
    onSelect(net);
    closeDialog();
  }

  function handleSchematicClick(event: MouseEvent) {
    const selectable = (event.target as Element | null)?.closest<SVGElement>("[data-net]");
    if (!selectable || !schematicHost?.contains(selectable)) return;
    const net = selectable.dataset.net;
    if (!net) return;
    if (!allowedNetNames.includes(net)) {
      unavailableNet = net;
      return;
    }
    chooseNetwork(net);
  }

  $effect(() => {
    if (!dialog) return;
    if (open && !dialog.open) {
      unavailableNet = "";
      zoom = 1;
      dialog.showModal();
      requestAnimationFrame(() => {
        syncViewportSize();
        syncHighlight();
        if (schematicHost) centerCanvasNow(schematicHost);
      });
    } else if (!open && dialog.open) {
      dialog.close();
    }
  });

  $effect(() => {
    selectedNet;
    schematicSvg;
    queueMicrotask(syncHighlight);
  });

  onDestroy(() => {
    pointerPan.destroy();
    wheelZoom.destroy();
  });
</script>

<dialog bind:this={dialog} class="modal" onclose={() => (open = false)}>
  <div class="modal-box flex h-[85vh] w-11/12 max-w-7xl flex-col gap-3 overflow-hidden p-4">
    <header class="flex shrink-0 items-start justify-between gap-4">
      <div class="min-w-0">
        <h2 class="text-lg font-bold">{title}</h2>
        <p class="mt-1 text-sm text-base-content/60">{ui.step2.netPickerHint}</p>
      </div>
      <div class="flex shrink-0 items-center gap-2">
        <ZoomControls {zoom} onZoom={setZoom} onReset={resetDiagram} />
        <button class="btn btn-sm btn-circle btn-ghost" type="button" onclick={closeDialog} aria-label={ui.common.close}>✕</button>
      </div>
    </header>

    {#if selectedNet}
      <div class="flex shrink-0 items-center gap-2 text-sm">
        <span class="text-base-content/60">{ui.step2.currentBinding}</span>
        <span class="badge badge-accent max-w-full truncate font-mono">{selectedNet}</span>
      </div>
    {/if}

    {#if unavailableNet}
      <div class="alert alert-warning shrink-0 py-2 text-sm" role="alert">
        <span>{ui.step2.netUnavailable(unavailableNet)}</span>
      </div>
    {/if}

    {#if open && schematicSvg}
      <div
        class="schematic-host min-h-0 flex-1 select-none overflow-auto rounded-box border border-base-300 bg-base-200 p-3"
        data-theme="nord"
        bind:this={schematicHost}
        use:centerCanvas
        use:observeViewport
        onclick={handleSchematicClick}
        onwheel={handleZoomWheel}
        onpointerdown={startPan}
        onpointermove={movePan}
        onpointerup={stopPan}
        onpointercancel={stopPan}
        onlostpointercapture={stopPan}
        oncontextmenu={(event) => event.preventDefault()}
        title={ui.step4.scrollHint}
        role="presentation"
      >
        <div
          class="schematic-stage"
          style:width={viewportWidth > 0
            ? `calc(100% + ${Math.max(1, viewportWidth - 24) * zoom}px)`
            : `${(zoom + 1) * 100}%`}
          style:height={viewportHeight > 0
            ? `calc(100% + ${Math.max(1, viewportHeight - 24) * zoom}px)`
            : `${(zoom + 1) * 100}%`}
        >
          <div
            class="schematic-content"
            style:width={viewportWidth > 0
              ? `${Math.max(1, viewportWidth - 24) * zoom}px`
              : `${(zoom / (zoom + 1)) * 100}%`}
            style:height={viewportHeight > 0
              ? `${Math.max(1, viewportHeight - 24) * zoom}px`
              : `${(zoom / (zoom + 1)) * 100}%`}
          >
            {@html schematicSvg}
          </div>
        </div>
      </div>
    {:else}
      <div class="hero grid min-h-0 flex-1 place-items-center rounded-box bg-base-200 p-6 text-center text-sm text-base-content/60">
        {ui.common.noSchematic}
      </div>
    {/if}

    <div class="modal-action mt-0 shrink-0">
      <button class="btn btn-ghost" type="button" onclick={() => chooseNetwork("")}>{ui.step2.clearBinding}</button>
      <button class="btn" type="button" onclick={closeDialog}>{ui.common.cancel}</button>
    </div>
  </div>
  <form method="dialog" class="modal-backdrop">
    <button aria-label={ui.common.close}>{ui.common.close}</button>
  </form>
</dialog>

<style>
  :global(.schematic-host) {
    contain: paint;
    overscroll-behavior: contain;
  }

  :global(.schematic-host.is-panning) {
    cursor: grabbing !important;
    user-select: none;
    will-change: scroll-position;
  }

  :global(.schematic-host.is-panning > *) {
    pointer-events: none;
  }

  .schematic-stage {
    display: grid;
    min-width: 100%;
    min-height: 100%;
    place-items: center;
  }

  :global(.schematic-content > svg) {
    display: block;
    width: 100% !important;
    height: 100% !important;
  }

  :global(.schematic-host [data-net]) {
    cursor: pointer;
    transition: opacity 160ms ease, filter 160ms ease, stroke 160ms ease;
  }

  :global(.schematic-host .is-muted) {
    opacity: 0.5;
  }

  :global(.schematic-host .sch-component.is-selected) {
    filter: drop-shadow(0 0 5px var(--color-highlight)) drop-shadow(0 0 2px var(--color-highlight));
  }

  :global(.schematic-host .sch-component.is-selected .sch-component-hit) {
    fill: var(--color-highlight);
    fill-opacity: 0.12;
    stroke: var(--color-highlight);
    stroke-width: 3;
    vector-effect: non-scaling-stroke;
  }

  :global(.schematic-host .sch-net-line.is-selected) {
    stroke: var(--color-highlight);
    stroke-width: 4;
    filter: drop-shadow(0 0 2px var(--color-highlight));
  }
</style>
