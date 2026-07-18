<script lang="ts">
  import { tick } from "svelte";
  import { centerCanvas, centerCanvasNow } from "$lib/actions/centerCanvas";
  import { ui } from "$lib/i18n";
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

  type PanGesture = {
    pointerId: number;
    startX: number;
    startY: number;
    startScrollLeft: number;
    startScrollTop: number;
  };

  let panGesture: PanGesture | null = null;

  function clampZoom(nextZoom: number) {
    return Math.min(3, Math.max(0.5, Math.round(nextZoom * 100) / 100));
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

  async function handleZoomWheel(event: WheelEvent) {
    event.preventDefault();
    const viewport = event.currentTarget as HTMLDivElement;
    const diagram = viewport.querySelector("svg");
    if (!diagram) return;

    const nextZoom = clampZoom(zoom * (event.deltaY < 0 ? 1.15 : 1 / 1.15));
    if (nextZoom === zoom) return;

    const before = diagram.getBoundingClientRect();
    const focusX = (event.clientX - before.left) / before.width;
    const focusY = (event.clientY - before.top) / before.height;
    await setZoom(nextZoom);

    const after = diagram.getBoundingClientRect();
    viewport.scrollLeft += after.left + focusX * after.width - event.clientX;
    viewport.scrollTop += after.top + focusY * after.height - event.clientY;
  }

  function startPan(event: PointerEvent) {
    if (event.button !== 2) return;
    event.preventDefault();
    const viewport = event.currentTarget as HTMLDivElement;
    panGesture = {
      pointerId: event.pointerId,
      startX: event.clientX,
      startY: event.clientY,
      startScrollLeft: viewport.scrollLeft,
      startScrollTop: viewport.scrollTop,
    };
    viewport.setPointerCapture(event.pointerId);
    viewport.classList.add("is-panning");
  }

  function movePan(event: PointerEvent) {
    if (!panGesture || event.pointerId !== panGesture.pointerId) return;
    event.preventDefault();
    const viewport = event.currentTarget as HTMLDivElement;
    viewport.scrollLeft = panGesture.startScrollLeft - (event.clientX - panGesture.startX);
    viewport.scrollTop = panGesture.startScrollTop - (event.clientY - panGesture.startY);
  }

  function stopPan(event: PointerEvent) {
    if (!panGesture || event.pointerId !== panGesture.pointerId) return;
    const viewport = event.currentTarget as HTMLDivElement;
    panGesture = null;
    viewport.classList.remove("is-panning");
    if (viewport.hasPointerCapture(event.pointerId)) viewport.releasePointerCapture(event.pointerId);
  }

  function syncHighlight() {
    if (!schematicHost) return;
    for (const element of schematicHost.querySelectorAll<SVGElement>(".sch-net-line")) {
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
  :global(.schematic-host.is-panning),
  :global(.schematic-host.is-panning *) {
    cursor: grabbing !important;
    user-select: none;
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

  :global(.schematic-host .sch-net-line.is-muted) {
    opacity: 0.5;
  }

  :global(.schematic-host .sch-net-line.is-selected) {
    stroke: var(--color-highlight);
    stroke-width: 4;
    filter: drop-shadow(0 0 2px var(--color-highlight));
  }
</style>
