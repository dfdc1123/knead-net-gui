<script lang="ts">
  import { tick } from "svelte";
  import { centerCanvas, centerCanvasNow } from "$lib/actions/centerCanvas";
  import { ui } from "$lib/i18n";
  import BreadboardPreview from "./BreadboardPreview.svelte";
  import ZoomControls from "./ZoomControls.svelte";
  import type {
    BreadboardHole,
    BreadboardPreset,
    CircuitSelection,
    LayoutFrame,
    LayoutPart,
    LayoutPin,
    LayoutWire,
  } from "$lib/layout";
  import { parseKiCadTextMarkup } from "$lib/layout";

  let {
    preset,
    upperHalfOnly = false,
    frame,
    schematicSvg = "",
  }: {
    preset: BreadboardPreset;
    upperHalfOnly?: boolean;
    frame: LayoutFrame;
    schematicSvg?: string;
  } = $props();

  let selected = $state<CircuitSelection | null>(null);
  let completedPartIds = $state<string[]>([]);
  let completedWireIds = $state<string[]>([]);
  let schematicHost = $state<HTMLDivElement>();
  let breadboardHost = $state<HTMLDivElement>();
  let assemblyListHost = $state<HTMLDivElement>();
  let resultLayoutHost = $state<HTMLDivElement>();
  let visualPanelsHost = $state<HTMLDivElement>();
  let wireListOpen = $state(true);
  let activeFrame = $state<LayoutFrame | null>(null);
  let schematicZoom = $state(1);
  let breadboardZoom = $state(1);
  let schematicViewportWidth = $state(0);
  let schematicViewportHeight = $state(0);
  let breadboardViewportWidth = $state(0);
  let breadboardViewportHeight = $state(0);
  let assemblyPanelWidth = $state(360);
  let schematicPanelHeight = $state<number | null>(null);

  type PanGesture = {
    pointerId: number;
    startX: number;
    startY: number;
    startScrollLeft: number;
    startScrollTop: number;
  };

  let panGesture: PanGesture | null = null;

  const MIN_ASSEMBLY_PANEL_WIDTH = 320;
  const MIN_CONTENT_WIDTH = 320;
  const MIN_VISUAL_PANEL_HEIGHT = 160;

  type PanelResizeGesture = {
    pointerId: number;
    startX: number;
    startWidth: number;
  };

  let panelResizeGesture: PanelResizeGesture | null = null;

  type VisualPanelResizeGesture = {
    pointerId: number;
    startY: number;
    startHeight: number;
  };

  let visualPanelResizeGesture: VisualPanelResizeGesture | null = null;

  type DiagramTarget = "schematic" | "breadboard";
  type PendingViewportSize = {
    viewport: HTMLDivElement;
    width: number;
    height: number;
  };

  let pendingViewportSizes: Partial<Record<DiagramTarget, PendingViewportSize>> = {};

  const breadboardRegionOrder: Record<BreadboardHole["region"], number> = {
    "rail-top": 0,
    "main-top": 1,
    "main-bottom": 2,
    "rail-bottom": 3,
  };

  function compareHoles(left: BreadboardHole, right: BreadboardHole) {
    return (
      left.col - right.col ||
      breadboardRegionOrder[left.region] - breadboardRegionOrder[right.region] ||
      left.row - right.row
    );
  }

  function firstHole(holes: BreadboardHole[]) {
    return holes.reduce<BreadboardHole | undefined>(
      (first, hole) => (!first || compareHoles(hole, first) < 0 ? hole : first),
      undefined,
    );
  }

  function compareParts(left: LayoutPart, right: LayoutPart) {
    const leftHole = firstHole(left.pins.map((pin) => pin.hole));
    const rightHole = firstHole(right.pins.map((pin) => pin.hole));
    if (leftHole && rightHole) {
      const positionOrder = compareHoles(leftHole, rightHole);
      if (positionOrder !== 0) return positionOrder;
    } else if (leftHole) {
      return -1;
    } else if (rightHole) {
      return 1;
    }
    return left.reference.localeCompare(right.reference) || left.id.localeCompare(right.id);
  }

  function orderedWireHoles(wire: LayoutWire) {
    return compareHoles(wire.from, wire.to) <= 0
      ? [wire.from, wire.to]
      : [wire.to, wire.from];
  }

  function compareWires(left: LayoutWire, right: LayoutWire) {
    const [leftStart, leftEnd] = orderedWireHoles(left);
    const [rightStart, rightEnd] = orderedWireHoles(right);
    return (
      compareHoles(leftStart, rightStart) ||
      compareHoles(leftEnd, rightEnd) ||
      left.id.localeCompare(right.id)
    );
  }

  let allWires = $derived(frame.wires ?? []);
  let selectedPart = $derived(
    selected?.type === "component"
      ? frame.parts.find((part) => part.reference === selected?.id)
      : undefined,
  );
  let parts = $derived([...frame.parts].sort(compareParts));
  let wires = $derived(allWires.filter((wire) => wire.kind !== "air").sort(compareWires));
  let assemblyParts = $derived([
    ...parts.filter((part) => !completedPartIds.includes(part.id)),
    ...parts.filter((part) => completedPartIds.includes(part.id)),
  ]);
  let assemblyWires = $derived([
    ...wires.filter((wire) => !completedWireIds.includes(wire.id)),
    ...wires.filter((wire) => completedWireIds.includes(wire.id)),
  ]);
  let wireNumbers = $derived(new Map(wires.map((wire, index) => [wire.id, index + 1])));
  let netCount = $derived(new Set(allWires.map((wire) => wire.net_id).filter(Boolean)).size);
  let completedPartCount = $derived(frame.parts.filter((part) => completedPartIds.includes(part.id)).length);
  let completedWireCount = $derived(wires.filter((wire) => completedWireIds.includes(wire.id)).length);
  let taskCount = $derived(frame.parts.length + wires.length);
  let completedTaskCount = $derived(completedPartCount + completedWireCount);
  let assemblyProgress = $derived(taskCount === 0 ? 0 : Math.round((completedTaskCount / taskCount) * 100));

  function choose(next: CircuitSelection | null) {
    selected =
      next && selected?.type === next.type && selected.id === next.id
        ? null
        : next;
  }

  function clampZoom(zoom: number) {
    return Math.min(3, Math.max(0.5, Math.round(zoom * 100) / 100));
  }

  function syncViewportSize(target: DiagramTarget) {
    const viewport = target === "schematic" ? schematicHost : breadboardHost;
    if (!viewport) return;
    const bounds = viewport.getBoundingClientRect();
    applyViewportSize(target, Math.round(bounds.width), Math.round(bounds.height));
  }

  function setDiagramZoom(zoom: number, target: DiagramTarget) {
    if (zoom === 1) syncViewportSize(target);
    if (target === "schematic") schematicZoom = zoom;
    else breadboardZoom = zoom;
  }

  function applyViewportSize(target: DiagramTarget, width: number, height: number) {
    if (target === "schematic") {
      schematicViewportWidth = width;
      schematicViewportHeight = height;
    } else {
      breadboardViewportWidth = width;
      breadboardViewportHeight = height;
    }
  }

  function layoutResizeActive() {
    return panelResizeGesture !== null || visualPanelResizeGesture !== null;
  }

  function centerFittedViewport(viewport: HTMLDivElement, target: DiagramTarget) {
    requestAnimationFrame(async () => {
      await tick();
      const zoom = target === "schematic" ? schematicZoom : breadboardZoom;
      if (zoom === 1) centerCanvasNow(viewport);
    });
  }

  function flushPendingViewportSizes() {
    for (const target of ["schematic", "breadboard"] as const) {
      const pending = pendingViewportSizes[target];
      if (!pending) continue;
      const zoom = target === "schematic" ? schematicZoom : breadboardZoom;
      if (zoom === 1) {
        applyViewportSize(target, pending.width, pending.height);
        centerFittedViewport(pending.viewport, target);
      }
    }
    pendingViewportSizes = {};
  }

  function observeDiagramViewport(viewport: HTMLDivElement, target: DiagramTarget) {
    let animationFrame = 0;
    const update = () => {
      const bounds = viewport.getBoundingClientRect();
      const size = {
        viewport,
        width: Math.round(bounds.width),
        height: Math.round(bounds.height),
      };
      if (layoutResizeActive()) {
        pendingViewportSizes[target] = size;
        const zoom = target === "schematic" ? schematicZoom : breadboardZoom;
        if (zoom === 1) {
          cancelAnimationFrame(animationFrame);
          animationFrame = requestAnimationFrame(() => centerCanvasNow(viewport));
        }
        return;
      }
      const zoom = target === "schematic" ? schematicZoom : breadboardZoom;
      if (zoom !== 1) return;
      applyViewportSize(target, size.width, size.height);
      cancelAnimationFrame(animationFrame);
      animationFrame = requestAnimationFrame(() => centerFittedViewport(viewport, target));
    };
    const resizeObserver = new ResizeObserver(update);
    resizeObserver.observe(viewport);
    update();

    return {
      destroy() {
        cancelAnimationFrame(animationFrame);
        resizeObserver.disconnect();
      },
    };
  }

  function maxAssemblyPanelWidth() {
    const layoutWidth = resultLayoutHost?.getBoundingClientRect().width ?? 0;
    return Math.max(MIN_ASSEMBLY_PANEL_WIDTH, layoutWidth - MIN_CONTENT_WIDTH - 12);
  }

  function clampAssemblyPanelWidth(width: number) {
    return Math.round(Math.min(maxAssemblyPanelWidth(), Math.max(MIN_ASSEMBLY_PANEL_WIDTH, width)));
  }

  function startAssemblyResize(event: PointerEvent) {
    if (event.button !== 0) return;
    panelResizeGesture = {
      pointerId: event.pointerId,
      startX: event.clientX,
      startWidth: assemblyPanelWidth,
    };
    (event.currentTarget as HTMLButtonElement).setPointerCapture(event.pointerId);
    event.preventDefault();
  }

  function resizeAssemblyPanel(event: PointerEvent) {
    if (!panelResizeGesture || event.pointerId !== panelResizeGesture.pointerId) return;
    assemblyPanelWidth = clampAssemblyPanelWidth(panelResizeGesture.startWidth - (event.clientX - panelResizeGesture.startX));
  }

  function stopAssemblyResize(event: PointerEvent) {
    if (!panelResizeGesture || event.pointerId !== panelResizeGesture.pointerId) return;
    const handle = event.currentTarget as HTMLButtonElement;
    if (handle.hasPointerCapture(event.pointerId)) {
      handle.releasePointerCapture(event.pointerId);
    }
    panelResizeGesture = null;
    flushPendingViewportSizes();
  }

  function resizeAssemblyPanelWithKeyboard(event: KeyboardEvent) {
    if (event.key === "ArrowLeft") {
      assemblyPanelWidth = clampAssemblyPanelWidth(assemblyPanelWidth + 24);
    } else if (event.key === "ArrowRight") {
      assemblyPanelWidth = clampAssemblyPanelWidth(assemblyPanelWidth - 24);
    } else if (event.key === "Home") {
      assemblyPanelWidth = MIN_ASSEMBLY_PANEL_WIDTH;
    } else if (event.key === "End") {
      assemblyPanelWidth = maxAssemblyPanelWidth();
    } else {
      return;
    }
    event.preventDefault();
  }

  function maxSchematicPanelHeight() {
    const layoutHeight = visualPanelsHost?.getBoundingClientRect().height ?? 0;
    return Math.max(MIN_VISUAL_PANEL_HEIGHT, layoutHeight - MIN_VISUAL_PANEL_HEIGHT - 12);
  }

  function clampSchematicPanelHeight(height: number) {
    return Math.round(Math.min(maxSchematicPanelHeight(), Math.max(MIN_VISUAL_PANEL_HEIGHT, height)));
  }

  function startVisualPanelResize(event: PointerEvent) {
    if (event.button !== 0 || !visualPanelsHost) return;
    const firstPanel = visualPanelsHost.firstElementChild;
    if (!firstPanel) return;
    visualPanelResizeGesture = {
      pointerId: event.pointerId,
      startY: event.clientY,
      startHeight: firstPanel.getBoundingClientRect().height,
    };
    (event.currentTarget as HTMLButtonElement).setPointerCapture(event.pointerId);
    event.preventDefault();
  }

  function resizeVisualPanels(event: PointerEvent) {
    if (!visualPanelResizeGesture || event.pointerId !== visualPanelResizeGesture.pointerId) return;
    schematicPanelHeight = clampSchematicPanelHeight(visualPanelResizeGesture.startHeight + (event.clientY - visualPanelResizeGesture.startY));
  }

  function stopVisualPanelResize(event: PointerEvent) {
    if (!visualPanelResizeGesture || event.pointerId !== visualPanelResizeGesture.pointerId) return;
    const handle = event.currentTarget as HTMLButtonElement;
    if (handle.hasPointerCapture(event.pointerId)) {
      handle.releasePointerCapture(event.pointerId);
    }
    visualPanelResizeGesture = null;
    flushPendingViewportSizes();
  }

  function resizeVisualPanelsWithKeyboard(event: KeyboardEvent) {
    const currentHeight = schematicPanelHeight ?? visualPanelsHost?.firstElementChild?.getBoundingClientRect().height;
    if (currentHeight === undefined) return;
    if (event.key === "ArrowUp") {
      schematicPanelHeight = clampSchematicPanelHeight(currentHeight - 24);
    } else if (event.key === "ArrowDown") {
      schematicPanelHeight = clampSchematicPanelHeight(currentHeight + 24);
    } else if (event.key === "Home") {
      schematicPanelHeight = MIN_VISUAL_PANEL_HEIGHT;
    } else if (event.key === "End") {
      schematicPanelHeight = maxSchematicPanelHeight();
    } else {
      return;
    }
    event.preventDefault();
  }

  async function handleZoomWheel(event: WheelEvent, target: "schematic" | "breadboard") {
    event.preventDefault();
    const viewport = event.currentTarget as HTMLDivElement;
    const diagram = viewport.querySelector("svg");
    if (!diagram) return;

    const currentZoom = target === "schematic" ? schematicZoom : breadboardZoom;
    const nextZoom = clampZoom(currentZoom * (event.deltaY < 0 ? 1.15 : 1 / 1.15));
    if (nextZoom === currentZoom) return;

    // 记录鼠标在图中的相对位置，更新尺寸后把同一点移回鼠标下方。
    const before = diagram.getBoundingClientRect();
    const focusX = (event.clientX - before.left) / before.width;
    const focusY = (event.clientY - before.top) / before.height;

    setDiagramZoom(nextZoom, target);
    await tick();

    const after = diagram.getBoundingClientRect();
    viewport.scrollLeft += after.left + focusX * after.width - event.clientX;
    viewport.scrollTop += after.top + focusY * after.height - event.clientY;
  }

  async function resetDiagram(target: "schematic" | "breadboard") {
    const viewport = target === "schematic" ? schematicHost : breadboardHost;
    syncViewportSize(target);
    setDiagramZoom(1, target);
    await tick();

    if (viewport) centerCanvasNow(viewport);
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

  function choosePart(part: LayoutPart) {
    choose({ type: "component", id: part.reference, label: part.reference });
  }

  function chooseWire(wire: LayoutWire) {
    choose({
      type: "wire",
      id: wire.id,
      label: wire.net_name || wire.net_id || wire.id,
      netId: wire.net_id,
    });
  }

  function setPartCompleted(id: string, completed: boolean) {
    completedPartIds = completed
      ? [...new Set([...completedPartIds, id])]
      : completedPartIds.filter((partId) => partId !== id);
  }

  function markAllParts(completed: boolean) {
    completedPartIds = completed ? frame.parts.map((part) => part.id) : [];
  }

  function setWireCompleted(id: string, completed: boolean) {
    completedWireIds = completed
      ? [...new Set([...completedWireIds, id])]
      : completedWireIds.filter((wireId) => wireId !== id);
  }

  function markAllWires(completed: boolean) {
    completedWireIds = completed ? wires.map((wire) => wire.id) : [];
  }

  function holeLabel(hole: BreadboardHole) {
    const column = hole.col + 1;
    if (hole.region === "main-top") return `${String.fromCharCode(65 + hole.row)}${column}`;
    if (hole.region === "main-bottom") return `${String.fromCharCode(70 + hole.row)}${column}`;
    const position = hole.region === "rail-top" ? ui.step4.top : ui.step4.bottom;
    const polarity = hole.row === 0 ? "−" : "+";
    return `${position}${polarity}${column}`;
  }

  function partPlacementSummary(part: LayoutPart) {
    const namedPin = (name: string) => part.pins.find((pin) => pin.name?.trim().toUpperCase() === name);
    if (part.device === "diode" || part.device === "led") {
      const cathode = namedPin("K");
      const anode = namedPin("A");
      if (cathode && anode) {
        return `${cathode.name}(${cathode.number}) ${holeLabel(cathode.hole)} · ${anode.name}(${anode.number}) ${holeLabel(anode.hole)}`;
      }
    }
    const pinOne = part.pins.find((pin) => pin.number === "1");
    return pinOne ? `1: ${holeLabel(pinOne.hole)}` : ui.common.placeholder;
  }

  function orderedPins(part: LayoutPart) {
    return [...part.pins].sort((left, right) => {
      const leftNumber = Number.parseInt(left.number ?? "", 10);
      const rightNumber = Number.parseInt(right.number ?? "", 10);
      if (Number.isFinite(leftNumber) && Number.isFinite(rightNumber)) {
        return leftNumber - rightNumber;
      }
      return (left.number ?? "").localeCompare(right.number ?? "");
    });
  }

  function netLabel(pin: LayoutPin) {
    return pin.net_name || pin.net_id || ui.common.placeholder;
  }

  function selectionFromElement(element: Element | null): CircuitSelection | null {
    const selectable = element?.closest<SVGElement>("[data-component], [data-net]");
    if (!selectable || !schematicHost?.contains(selectable)) return null;
    const component = selectable.dataset.component;
    if (component) return { type: "component", id: component, label: component };
    const net = selectable.dataset.net;
    if (net) return { type: "net", id: net, label: net };
    return null;
  }

  function handleSchematicClick(event: MouseEvent) {
    choose(selectionFromElement(event.target as Element));
  }

  function syncSchematicHighlight() {
    if (!schematicHost) return;
    for (const element of schematicHost.querySelectorAll<SVGElement>("[data-component]")) {
      const active = selected?.type === "component" && element.dataset.component === selected.id;
      element.classList.toggle("is-selected", active);
      element.classList.toggle("is-muted", selected?.type === "component" && !active);
    }
    for (const element of schematicHost.querySelectorAll<SVGElement>(".sch-net-line")) {
      const selectedNet = selected?.type === "net" ? selected.id : selected?.type === "wire" ? selected.netId : undefined;
      const active = selectedNet !== undefined && element.dataset.net === selectedNet;
      element.classList.toggle("is-selected", active);
      element.classList.toggle("is-muted", selectedNet !== undefined && !active);
    }
  }

  async function revealWireInAssemblyList(wireId: string) {
    await tick();
    const row = [...(assemblyListHost?.querySelectorAll<HTMLElement>("[data-wire-id]") ?? [])]
      .find((candidate) => candidate.dataset.wireId === wireId);
    row?.scrollIntoView({ block: "nearest", inline: "nearest" });
  }

  $effect(() => {
    selected;
    schematicSvg;
    queueMicrotask(syncSchematicHighlight);
  });

  $effect(() => {
    const wireId = selected?.type === "wire" ? selected.id : null;
    if (!wireId || !assemblyListHost) return;
    wireListOpen = true;
    void revealWireInAssemblyList(wireId);
  });

  $effect(() => {
    if (activeFrame === null) {
      activeFrame = frame;
      return;
    }
    if (frame !== activeFrame) {
      activeFrame = frame;
      completedPartIds = [];
      completedWireIds = [];
      selected = null;
    }
  });
</script>

<div class="mx-auto flex h-full min-h-0 w-full max-w-[1920px] flex-col gap-3 overflow-hidden p-4">
  <header class="flex shrink-0 items-center justify-between gap-3">
    <div>
      <h1 class="text-2xl font-bold">{ui.step4.title}</h1>
      <p class="text-sm text-base-content/60">{ui.step4.subtitle}</p>
    </div>

    <div class="flex items-center gap-2">
      <div class="join">
        <span class="badge badge-outline join-item h-8">{ui.step4.componentCount(frame.parts.length)}</span>
        <span class="badge badge-outline join-item h-8">{ui.step4.wireCount(wires.length)}</span>
        <span class="badge badge-outline join-item h-8">{ui.step4.netCount(netCount)}</span>
      </div>
      {#if selected}
        <button class="btn btn-sm btn-ghost" onclick={() => (selected = null)} aria-label={ui.step4.clearHighlight}>{ui.step4.clearHighlight}</button>
      {/if}
    </div>
  </header>

  <div
    class="alert h-10 shrink-0 overflow-hidden py-2 text-sm {selected ? 'alert-warning' : 'border border-base-300 bg-base-100 text-base-content/60'}"
    aria-live="polite"
  >
    <span class="status {selected ? 'status-warning' : 'status-neutral'}" aria-hidden="true"></span>
    {#if selected}
      <span>
        <span class="badge badge-sm {selected.type === 'component' ? 'badge-primary' : selected.type === 'wire' ? 'badge-accent' : 'badge-secondary'}">
          {selected.type === "component" ? ui.step4.component : selected.type === "wire" ? ui.step4.wire : ui.step4.net}
        </span>
        <strong class="ml-1 font-mono">{selected.label}</strong>
        {#if selectedPart}
          <span class="ml-2 text-xs opacity-70">
            {selectedPart.value || ui.common.placeholder} · {selectedPart.description || ui.step4.pinCount(selectedPart.pins.length)}
          </span>
        {/if}
      </span>
    {:else}
      <span>{ui.step4.selectionHint}</span>
    {/if}
  </div>

  <div
    class="grid min-h-0 flex-1 grid-cols-[minmax(0,1fr)_0.75rem_minmax(20rem,1fr)]"
    bind:this={resultLayoutHost}
    style:grid-template-columns={`minmax(0, 1fr) 0.75rem ${assemblyPanelWidth}px`}
  >
    <div
      class="grid min-h-0 grid-rows-[minmax(0,1fr)_0.75rem_minmax(0,1fr)]"
      bind:this={visualPanelsHost}
      style:grid-template-rows={schematicPanelHeight === null ? undefined : `${schematicPanelHeight}px 0.75rem minmax(0, 1fr)`}
    >
      <section class="card min-h-0 overflow-hidden border border-base-300 bg-base-100 shadow-sm">
        <div class="card-body min-h-0 gap-2 p-3">
          <div class="flex shrink-0 items-center justify-between px-1">
            <h2 class="card-title text-base">{ui.common.schematic}</h2>
            <div class="flex items-center gap-2">
              <span class="badge badge-ghost badge-sm">SCH</span>
              <ZoomControls
                zoom={schematicZoom}
                onZoom={(zoom) => setDiagramZoom(clampZoom(zoom), "schematic")}
                onReset={() => resetDiagram("schematic")}
              />
            </div>
          </div>
          {#if schematicSvg}
            <div
              class="diagram-viewport schematic-host min-h-0 flex-1 overflow-auto rounded-box border border-base-300 bg-base-200 p-3"
              bind:this={schematicHost}
              use:centerCanvas
              use:observeDiagramViewport={"schematic"}
              onclick={handleSchematicClick}
              onwheel={(event) => handleZoomWheel(event, "schematic")}
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
                style:width={schematicViewportWidth > 0
                  ? `calc(100% + ${Math.max(1, schematicViewportWidth - 24) * schematicZoom}px)`
                  : `${(schematicZoom + 1) * 100}%`}
                style:height={schematicViewportHeight > 0
                  ? `calc(100% + ${Math.max(1, schematicViewportHeight - 24) * schematicZoom}px)`
                  : `${(schematicZoom + 1) * 100}%`}
              >
                <div
                  class="schematic-content"
                  style:width={schematicViewportWidth > 0
                    ? `${Math.max(1, schematicViewportWidth - 24) * schematicZoom}px`
                    : `${(schematicZoom / (schematicZoom + 1)) * 100}%`}
                  style:height={schematicViewportHeight > 0
                    ? `${Math.max(1, schematicViewportHeight - 24) * schematicZoom}px`
                    : `${(schematicZoom / (schematicZoom + 1)) * 100}%`}
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
        </div>
      </section>

      <button
        type="button"
        class="group relative cursor-row-resize touch-none border-0 bg-transparent p-0 outline-none focus-visible:bg-primary/20"
        aria-label={ui.step4.resizeVisualPanels}
        title={ui.step4.resizeVisualPanels}
        onpointerdown={startVisualPanelResize}
        onpointermove={resizeVisualPanels}
        onpointerup={stopVisualPanelResize}
        onpointercancel={stopVisualPanelResize}
        onlostpointercapture={stopVisualPanelResize}
        onkeydown={resizeVisualPanelsWithKeyboard}
      >
        <div class="absolute inset-x-0 top-1/2 h-px -translate-y-1/2 bg-base-300 group-hover:bg-primary"></div>
      </button>

      <section class="card min-h-0 overflow-hidden border border-base-300 bg-base-100 shadow-sm">
        <div class="card-body min-h-0 gap-2 p-3">
          <div class="flex shrink-0 items-center justify-between gap-3 px-1">
            <div class="flex items-center gap-2">
              <h2 class="card-title text-base">{ui.step4.breadboard}</h2>
              <span class="badge badge-primary badge-sm">{ui.step4.boards(frame.board_count)}</span>
              <span class="badge badge-ghost badge-sm">{ui.step4.columns(frame.total_cols)}</span>
            </div>
            <div class="flex items-center gap-3 text-xs">
              <span class="flex items-center gap-1.5"><span class="status status-success"></span>{ui.step4.completedSolid}</span>
              <span class="flex items-center gap-1.5 text-base-content/60"><span class="status status-neutral"></span>{ui.step4.pendingDashed}</span>
              <ZoomControls
                zoom={breadboardZoom}
                onZoom={(zoom) => setDiagramZoom(clampZoom(zoom), "breadboard")}
                onReset={() => resetDiagram("breadboard")}
              />
            </div>
          </div>
          <div
            class="diagram-viewport min-h-0 flex-1 overflow-auto rounded-box border border-base-300 bg-base-200"
            bind:this={breadboardHost}
            use:centerCanvas
            use:observeDiagramViewport={"breadboard"}
            onwheel={(event) => handleZoomWheel(event, "breadboard")}
            onpointerdown={startPan}
            onpointermove={movePan}
            onpointerup={stopPan}
            onpointercancel={stopPan}
            onlostpointercapture={stopPan}
            oncontextmenu={(event) => event.preventDefault()}
            title={ui.step4.scrollHint}
            role="presentation"
          >
            <BreadboardPreview
              {preset}
              boardCols={frame.board_cols}
              boardCount={frame.board_count}
              gapCols={frame.gap_cols}
              {upperHalfOnly}
              {frame}
              zoom={breadboardZoom}
              fitWidth={breadboardViewportWidth}
              fitHeight={breadboardViewportHeight}
              panCanvas
              {selected}
              {completedWireIds}
              onSelect={choose}
            />
          </div>
        </div>
      </section>
    </div>

    <button
      type="button"
      class="group relative cursor-col-resize touch-none border-0 bg-transparent p-0 outline-none focus-visible:bg-primary/20"
      aria-label={ui.step4.resizeAssemblyList}
      title={ui.step4.resizeAssemblyList}
      onpointerdown={startAssemblyResize}
      onpointermove={resizeAssemblyPanel}
      onpointerup={stopAssemblyResize}
      onpointercancel={stopAssemblyResize}
      onlostpointercapture={stopAssemblyResize}
      onkeydown={resizeAssemblyPanelWithKeyboard}
    >
      <div class="absolute inset-y-0 left-1/2 w-px -translate-x-1/2 bg-base-300 group-hover:bg-primary"></div>
    </button>

    <aside class="card min-h-0 overflow-hidden border border-base-300 bg-base-100 shadow-sm" aria-label={ui.step4.assemblyList}>
      <div class="card-body min-h-0 gap-3 p-3">
        <div class="shrink-0 px-1">
          <div class="flex items-center justify-between gap-2">
            <h2 class="card-title text-base">{ui.step4.assemblyList}</h2>
            <span class="badge {completedTaskCount === taskCount && taskCount > 0 ? 'badge-success' : 'badge-primary'} badge-sm">
              {completedTaskCount} / {taskCount}
            </span>
          </div>
          <progress class="progress progress-primary mt-2 w-full" value={assemblyProgress} max="100" aria-label={`${ui.step4.assemblyProgress} ${assemblyProgress}%`}></progress>
          <div class="mt-1 flex items-center justify-between text-xs text-base-content/60">
            <span>{ui.step4.assemblyProgress}</span>
            <span>{assemblyProgress}%</span>
          </div>
        </div>

        {#if selectedPart}
          <section class="shrink-0 overflow-hidden rounded-box border border-warning/50 bg-warning/5" aria-label={ui.step4.selectedDetails}>
            <div class="flex items-start justify-between gap-2 border-b border-warning/30 px-3 py-2">
              <div class="min-w-0">
                <div class="flex items-center gap-2">
                  <span class="badge badge-warning badge-sm font-mono">{selectedPart.reference}</span>
                  <strong class="truncate text-sm">{selectedPart.value || ui.common.placeholder}</strong>
                </div>
                {#if selectedPart.description}
                  <p class="mt-1 truncate text-xs text-base-content/65" title={selectedPart.description}>{selectedPart.description}</p>
                {/if}
                <div class="mt-1 flex flex-wrap gap-1">
                  {#if selectedPart.dnp}<span class="badge badge-error badge-xs">{ui.step4.dnp}</span>{/if}
                  {#if selectedPart.in_bom !== undefined}<span class="badge badge-ghost badge-xs">{selectedPart.in_bom ? ui.step4.includedInBom : ui.step4.excludedFromBom}</span>{/if}
                  {#if selectedPart.on_board !== undefined}<span class="badge badge-ghost badge-xs">{selectedPart.on_board ? ui.step4.onBoard : ui.step4.offBoard}</span>{/if}
                  {#if selectedPart.in_pos_files !== undefined}<span class="badge badge-ghost badge-xs">{selectedPart.in_pos_files ? ui.step4.includedInPos : ui.step4.excludedFromPos}</span>{/if}
                  {#if selectedPart.exclude_from_sim !== undefined}<span class="badge badge-ghost badge-xs">{selectedPart.exclude_from_sim ? ui.step4.excludedFromSim : ui.step4.includedInSim}</span>{/if}
                </div>
              </div>
              <span class="badge badge-outline badge-sm shrink-0">{ui.step4.pinCount(selectedPart.pins.length)}</span>
            </div>
            <div class="max-h-56 overflow-auto px-2 py-1">
              <table class="table table-xs">
                <thead>
                  <tr>
                    <th>{ui.step4.number}</th>
                    <th>{ui.step4.nameTypeShape}</th>
                    <th>{ui.step4.net}</th>
                    <th>{ui.step4.hole}</th>
                  </tr>
                </thead>
                <tbody>
                  {#each orderedPins(selectedPart) as pin}
                    <tr>
                      <td class="font-mono font-semibold">{pin.number || ui.common.placeholder}</td>
                      <td>
                        <div class="flex flex-wrap items-center gap-1">
                          <span>
                            {#each parseKiCadTextMarkup(pin.name || ui.common.placeholder) as segment}
                              <span class:overline={segment.overbar}>{segment.text}</span>
                            {/each}
                          </span>
                          {#if pin.pin_type}<span class="badge badge-ghost badge-xs">{pin.pin_type}</span>{/if}
                          {#if pin.pin_shape}<span class="badge badge-ghost badge-xs">{pin.pin_shape}</span>{/if}
                        </div>
                        {#if pin.unit !== undefined}<span class="text-[0.65rem] text-base-content/50">{ui.step4.unit} {pin.unit}</span>{/if}
                      </td>
                      <td class="max-w-32 truncate font-mono text-[0.68rem]" title={netLabel(pin)}>{netLabel(pin)}</td>
                      <td class="whitespace-nowrap font-mono text-[0.68rem]">{holeLabel(pin.hole)}</td>
                    </tr>
                  {/each}
                </tbody>
              </table>
            </div>
            {#if selectedPart.properties?.length}
              <details class="border-t border-warning/30 px-3 py-2 text-xs">
                <summary class="cursor-pointer font-medium">{ui.step4.properties} ({selectedPart.properties.length})</summary>
                <dl class="mt-2 grid grid-cols-[max-content_minmax(0,1fr)] gap-x-2 gap-y-1">
                  {#each selectedPart.properties as property}
                    <dt class="font-mono text-base-content/60">{property.name}</dt>
                    <dd class="min-w-0 truncate" title={property.value}>
                      {#each parseKiCadTextMarkup(property.value) as segment}
                        <span class:overline={segment.overbar}>{segment.text}</span>
                      {/each}
                      {#if property.hidden}<span class="badge badge-ghost badge-xs ml-1">{ui.step4.hiddenProperty}</span>{/if}
                    </dd>
                  {/each}
                </dl>
              </details>
            {/if}
            <div class="flex flex-wrap gap-x-3 gap-y-1 border-t border-warning/30 px-3 py-2 text-[0.68rem] text-base-content/55">
              <span class="truncate" title={selectedPart.footprint}>{ui.step4.footprint}: {selectedPart.footprint}</span>
              {#if selectedPart.datasheet}<span class="truncate" title={selectedPart.datasheet}>{ui.step4.datasheet}: {selectedPart.datasheet}</span>{/if}
            </div>
          </section>
        {/if}

        <div class="min-h-0 flex-1 space-y-2 overflow-y-auto pr-1" bind:this={assemblyListHost}>
          <div class="collapse-arrow collapse border border-base-300 bg-base-100">
            <input type="checkbox" checked aria-label={ui.step4.toggleComponents} />
            <div class="collapse-title flex min-h-12 items-center gap-2 py-3 font-semibold">
              {ui.step4.components}
              <span class="badge {completedPartCount === frame.parts.length && frame.parts.length > 0 ? 'badge-success' : 'badge-neutral'} badge-sm">
                {completedPartCount} / {frame.parts.length}
              </span>
            </div>
            <div class="collapse-content px-2 pb-2">
              {#if frame.parts.length > 0}
                <div class="join mb-2 flex w-full">
                  <button class="btn btn-sm join-item flex-1" onclick={() => markAllParts(true)} disabled={completedPartCount === frame.parts.length}>{ui.step4.markAllComplete}</button>
                  <button class="btn btn-sm join-item flex-1" onclick={() => markAllParts(false)} disabled={completedPartCount === 0}>{ui.step4.resetAll}</button>
                </div>
                <ul class="overflow-hidden rounded-box border border-base-300 bg-base-100">
                  {#each assemblyParts as part (part.id)}
                    {@const completed = completedPartIds.includes(part.id)}
                    <li class="assembly-row relative grid grid-cols-[auto_1fr] items-center gap-2 border-b border-base-300 px-3 py-2 transition-colors last:border-b-0 hover:bg-base-200 {completed ? 'bg-success/10' : ''} {selected?.type === 'component' && selected.id === part.reference ? 'ring-1 ring-warning ring-inset' : ''}">
                      <button
                        class="assembly-row-hit absolute inset-0 cursor-pointer"
                        onclick={() => choosePart(part)}
                        aria-label={ui.step4.selectComponent(part.reference)}
                      ></button>
                      <input
                        class="checkbox checkbox-success checkbox-sm relative z-10 self-center"
                        type="checkbox"
                        checked={completed}
                        aria-label={completed ? ui.step4.markComponentPending(part.reference) : ui.step4.markComponentInstalled(part.reference)}
                        onchange={(event) => setPartCompleted(part.id, event.currentTarget.checked)}
                      />
                      <div class="pointer-events-none relative z-10 grid min-w-0 grid-cols-[auto_1fr] items-center gap-x-2">
                        <span class="badge badge-outline badge-sm row-span-2 font-mono">{part.reference}</span>
                        <span class="truncate text-sm font-medium {completed ? 'line-through opacity-60' : ''}">{part.value || ui.common.placeholder}</span>
                        <span class="truncate text-xs text-base-content/55">
                          {ui.step4.pinCount(part.pins.length)} · {partPlacementSummary(part)}
                        </span>
                      </div>
                    </li>
                  {/each}
                </ul>
              {:else}
                <div class="py-4 text-center text-sm text-base-content/50">{ui.step4.noComponents}</div>
              {/if}
            </div>
          </div>

          <div class="collapse-arrow collapse border border-base-300 bg-base-100">
            <input type="checkbox" bind:checked={wireListOpen} aria-label={ui.step4.toggleWires} />
            <div class="collapse-title flex min-h-12 items-center gap-2 py-3 font-semibold">
              {ui.step4.wires}
              <span class="badge {completedWireCount === wires.length && wires.length > 0 ? 'badge-success' : 'badge-neutral'} badge-sm">
                {completedWireCount} / {wires.length}
              </span>
            </div>
            <div class="collapse-content px-2 pb-2">
              {#if wires.length > 0}
                <div class="join mb-2 flex w-full">
                  <button class="btn btn-sm join-item flex-1" onclick={() => markAllWires(true)} disabled={completedWireCount === wires.length}>{ui.step4.markAllComplete}</button>
                  <button class="btn btn-sm join-item flex-1" onclick={() => markAllWires(false)} disabled={completedWireCount === 0}>{ui.step4.resetAll}</button>
                </div>
                <ul class="overflow-hidden rounded-box border border-base-300 bg-base-100">
                  {#each assemblyWires as wire (wire.id)}
                    {@const completed = completedWireIds.includes(wire.id)}
                    {@const wireNumber = wireNumbers.get(wire.id)}
                    <li
                      class="assembly-row relative grid grid-cols-[auto_1fr] items-center gap-2 border-b border-base-300 px-3 py-2 transition-colors last:border-b-0 hover:bg-base-200 {completed ? 'bg-success/10' : ''} {selected?.type === 'wire' && selected.id === wire.id ? 'ring-1 ring-warning ring-inset' : ''}"
                      data-wire-id={wire.id}
                    >
                      <button
                        class="assembly-row-hit absolute inset-0 cursor-pointer"
                        onclick={() => chooseWire(wire)}
                        aria-label={ui.step4.selectWire(wireNumber)}
                      ></button>
                      <input
                        class="checkbox checkbox-success checkbox-sm relative z-10 row-span-2 self-center"
                        type="checkbox"
                        checked={completed}
                        aria-label={completed ? ui.step4.markWirePending(wireNumber) : ui.step4.markWireComplete(wireNumber)}
                        onchange={(event) => setWireCompleted(wire.id, event.currentTarget.checked)}
                      />
                      <div class="pointer-events-none relative z-10 min-w-0">
                        <span class="flex items-center gap-2">
                          <span
                            class="status"
                            style:background-color={wire.color ?? "var(--color-primary)"}
                            aria-hidden="true"
                          ></span>
                          <span class="truncate text-sm font-medium {completed ? 'line-through opacity-60' : ''}">
                            {wire.kind === "rail-tie" ? ui.step4.railTieLabel(wireNumber) : ui.step4.wireLabel(wireNumber)} · {wire.net_name || wire.net_id || ui.common.placeholder}
                          </span>
                        </span>
                        <span class="mt-0.5 block font-mono text-xs text-base-content/55">
                          {holeLabel(wire.from)} → {holeLabel(wire.to)}
                        </span>
                      </div>
                    </li>
                  {/each}
                </ul>
              {:else}
                <div class="py-4 text-center text-sm text-base-content/50">{ui.step4.noWires}</div>
              {/if}
            </div>
          </div>
        </div>

        {#if taskCount > 0 && completedTaskCount === taskCount}
          <div class="alert alert-success shrink-0 py-2 text-sm" role="status">
            <span class="status status-success"></span>
            <span>{ui.step4.allComplete}</span>
          </div>
        {/if}
      </div>
    </aside>
  </div>
</div>

<style>
  :global(.diagram-viewport.is-panning),
  :global(.diagram-viewport.is-panning *) {
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

  .assembly-row:has(> .assembly-row-hit:focus-visible) {
    z-index: 1;
    outline: 2px solid var(--color-primary);
    outline-offset: -2px;
  }

  :global(.schematic-host .sch-component),
  :global(.schematic-host [data-net]) {
    cursor: pointer;
    transition: opacity 160ms ease, filter 160ms ease, stroke 160ms ease;
  }

  :global(.schematic-host .is-muted) {
    opacity: 0.16;
  }

  :global(.schematic-host .sch-component.is-selected) {
    filter: drop-shadow(0 0 5px var(--color-warning)) drop-shadow(0 0 2px var(--color-warning));
  }

  :global(.schematic-host .sch-net-line.is-selected) {
    stroke: var(--color-warning);
    stroke-width: 4;
    filter: drop-shadow(0 0 2px var(--color-warning));
  }
</style>
