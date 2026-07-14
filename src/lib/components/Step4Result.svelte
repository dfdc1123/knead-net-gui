<script lang="ts">
  import BreadboardPreview from "./BreadboardPreview.svelte";
  import type {
    BreadboardHole,
    BreadboardPreset,
    CircuitSelection,
    LayoutFrame,
    LayoutPart,
    LayoutWire,
  } from "$lib/layout";

  let {
    preset,
    cols,
    frame,
    schematicSvg = "",
  }: {
    preset: BreadboardPreset;
    cols: number;
    frame: LayoutFrame;
    schematicSvg?: string;
  } = $props();

  let selected = $state<CircuitSelection | null>(null);
  let completedWireIds = $state<string[]>([]);
  let schematicHost = $state<HTMLDivElement>();
  let activeFrame = $state<LayoutFrame | null>(null);

  let allWires = $derived(frame.wires ?? []);
  let wires = $derived(allWires.filter((wire) => wire.kind !== "air"));
  let netCount = $derived(new Set(allWires.map((wire) => wire.net_id).filter(Boolean)).size);
  let completedWireCount = $derived(wires.filter((wire) => completedWireIds.includes(wire.id)).length);
  let wireProgress = $derived(wires.length === 0 ? 0 : Math.round((completedWireCount / wires.length) * 100));

  function choose(next: CircuitSelection | null) {
    selected =
      next && selected?.type === next.type && selected.id === next.id
        ? null
        : next;
  }

  function choosePart(part: LayoutPart) {
    choose({ type: "component", id: part.reference, label: part.reference });
  }

  function chooseWire(wire: LayoutWire) {
    if (!wire.net_id) return;
    choose({ type: "net", id: wire.net_id, label: wire.net_name || wire.net_id });
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
    const position = hole.region === "rail-top" ? "上" : "下";
    const polarity = hole.row === 0 ? "−" : "+";
    return `${position}${polarity}${column}`;
  }

  function partKindLabel(part: LayoutPart) {
    if (part.kind === "ic") return "IC";
    if (part.kind === "axial") return "轴向";
    return "元件";
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
      const active = selected?.type === "net" && element.dataset.net === selected.id;
      element.classList.toggle("is-selected", active);
      element.classList.toggle("is-muted", selected?.type === "net" && !active);
    }
  }

  $effect(() => {
    selected;
    schematicSvg;
    queueMicrotask(syncSchematicHighlight);
  });

  $effect(() => {
    if (activeFrame === null) {
      activeFrame = frame;
      return;
    }
    if (frame !== activeFrame) {
      activeFrame = frame;
      completedWireIds = [];
      selected = null;
    }
  });
</script>

<div class="mx-auto flex h-full min-h-0 w-full max-w-screen-2xl flex-col gap-3 overflow-hidden p-4">
  <header class="flex shrink-0 items-center justify-between gap-3">
    <div>
      <h1 class="text-2xl font-bold">装配视图</h1>
      <p class="text-sm text-base-content/60">按右侧清单逐根接线，面包板会同步显示装配状态</p>
    </div>

    <div class="flex items-center gap-2">
      <div class="join">
        <span class="badge badge-outline join-item h-8">{frame.parts.length} 个元件</span>
        <span class="badge badge-outline join-item h-8">{wires.length} 根跳线</span>
        <span class="badge badge-outline join-item h-8">{netCount} 个网络</span>
      </div>
      {#if selected}
        <button class="btn btn-sm btn-ghost" onclick={() => (selected = null)} aria-label="清除高亮">清除高亮</button>
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
        <span class="badge badge-sm {selected.type === 'component' ? 'badge-primary' : 'badge-secondary'}">{selected.type === "component" ? "元件" : "网络"}</span>
        <strong class="ml-1 font-mono">{selected.label}</strong>
      </span>
    {:else}
      <span>点击原理图、面包板或清单中的条目，可同步查看对应关系</span>
    {/if}
  </div>

  <div class="grid min-h-0 flex-1 grid-cols-[minmax(0,2fr)_minmax(20rem,1fr)] gap-3">
    <div class="grid min-h-0 grid-rows-2 gap-3">
      <section class="card min-h-0 overflow-hidden border border-base-300 bg-base-100 shadow-sm">
        <div class="card-body min-h-0 gap-2 p-3">
          <div class="flex shrink-0 items-center justify-between px-1">
            <h2 class="card-title text-base">原理图</h2>
            <span class="badge badge-ghost badge-sm">SCH</span>
          </div>
          {#if schematicSvg}
            <div
              class="schematic-host min-h-0 flex-1 overflow-auto rounded-box border border-base-300 bg-base-200 p-3"
              bind:this={schematicHost}
              onclick={handleSchematicClick}
              role="presentation"
            >
              {@html schematicSvg}
            </div>
          {:else}
            <div class="hero grid min-h-0 flex-1 place-items-center rounded-box bg-base-200 p-6 text-center text-sm text-base-content/60">
              无原理图
            </div>
          {/if}
        </div>
      </section>

      <section class="card min-h-0 overflow-hidden border border-base-300 bg-base-100 shadow-sm">
        <div class="card-body min-h-0 gap-2 p-3">
          <div class="flex shrink-0 items-center justify-between gap-3 px-1">
            <div class="flex items-center gap-2">
              <h2 class="card-title text-base">面包板</h2>
              <span class="badge badge-ghost badge-sm">{cols} 列</span>
            </div>
            <div class="flex items-center gap-3 text-xs">
              <span class="flex items-center gap-1.5"><span class="status status-success"></span>已完成（实线）</span>
              <span class="flex items-center gap-1.5 text-base-content/60"><span class="status status-neutral"></span>待连接（虚线）</span>
            </div>
          </div>
          <div class="min-h-0 flex-1 overflow-auto rounded-box border border-base-300 bg-base-200">
            <BreadboardPreview
              {preset}
              {cols}
              {frame}
              {selected}
              {completedWireIds}
              onSelect={choose}
            />
          </div>
        </div>
      </section>
    </div>

    <aside class="card min-h-0 overflow-hidden border border-base-300 bg-base-100 shadow-sm" aria-label="装配清单">
      <div class="card-body min-h-0 gap-3 p-3">
        <div class="shrink-0 px-1">
          <div class="flex items-center justify-between gap-2">
            <h2 class="card-title text-base">装配清单</h2>
            <span class="badge {completedWireCount === wires.length && wires.length > 0 ? 'badge-success' : 'badge-primary'} badge-sm">
              {completedWireCount} / {wires.length}
            </span>
          </div>
          <progress class="progress progress-primary mt-2 w-full" value={wireProgress} max="100" aria-label="跳线完成进度 {wireProgress}%"></progress>
          <div class="mt-1 flex items-center justify-between text-xs text-base-content/60">
            <span>跳线完成进度</span>
            <span>{wireProgress}%</span>
          </div>
        </div>

        <div class="min-h-0 flex-1 space-y-2 overflow-y-auto pr-1">
          <div class="collapse-arrow collapse border border-base-300 bg-base-100">
            <input type="checkbox" checked aria-label="展开或收起元器件列表" />
            <div class="collapse-title flex min-h-12 items-center gap-2 py-3 font-semibold">
              元器件
              <span class="badge badge-neutral badge-sm">{frame.parts.length}</span>
            </div>
            <div class="collapse-content px-2 pb-2">
              {#if frame.parts.length > 0}
                <ul class="list rounded-box bg-base-200">
                  {#each frame.parts as part (part.id)}
                    <li class="list-row px-3 py-2 {selected?.type === 'component' && selected.id === part.reference ? 'bg-primary/10' : ''}">
                      <button class="grid min-w-0 flex-1 grid-cols-[auto_1fr] items-center gap-x-2 text-left" onclick={() => choosePart(part)}>
                        <span class="badge badge-outline badge-sm row-span-2 font-mono">{part.reference}</span>
                        <span class="truncate text-sm font-medium">{part.value || "未标注值"}</span>
                        <span class="truncate text-xs text-base-content/55">
                          {partKindLabel(part)} · {part.pins.length} 个引脚
                          {#if part.pins[0]} · {holeLabel(part.pins[0].hole)}{/if}
                        </span>
                      </button>
                    </li>
                  {/each}
                </ul>
              {:else}
                <div class="py-4 text-center text-sm text-base-content/50">无元器件</div>
              {/if}
            </div>
          </div>

          <div class="collapse-arrow collapse border border-base-300 bg-base-100">
            <input type="checkbox" checked aria-label="展开或收起跳线列表" />
            <div class="collapse-title flex min-h-12 items-center gap-2 py-3 font-semibold">
              跳线
              <span class="badge badge-neutral badge-sm">{wires.length}</span>
            </div>
            <div class="collapse-content px-2 pb-2">
              {#if wires.length > 0}
                <div class="join mb-2 flex w-full">
                  <button class="btn btn-sm join-item flex-1" onclick={() => markAllWires(true)} disabled={completedWireCount === wires.length}>全部完成</button>
                  <button class="btn btn-sm join-item flex-1" onclick={() => markAllWires(false)} disabled={completedWireCount === 0}>全部重置</button>
                </div>
                <ul class="list rounded-box bg-base-200">
                  {#each wires as wire, index (wire.id)}
                    {@const completed = completedWireIds.includes(wire.id)}
                    <li class="list-row grid-cols-[auto_1fr] gap-2 px-3 py-2 {completed ? 'bg-success/10' : ''} {selected?.type === 'net' && selected.id === wire.net_id ? 'ring-1 ring-warning ring-inset' : ''}">
                      <input
                        class="checkbox checkbox-success checkbox-sm row-span-2 self-center"
                        type="checkbox"
                        checked={completed}
                        aria-label="{completed ? '标记为待连接' : '标记为已完成'}：跳线 {index + 1}"
                        onchange={(event) => setWireCompleted(wire.id, event.currentTarget.checked)}
                      />
                      <button class="min-w-0 text-left" onclick={() => chooseWire(wire)}>
                        <span class="flex items-center gap-2">
                          <span
                            class="status"
                            style:background-color={wire.color ?? "var(--color-primary)"}
                            aria-hidden="true"
                          ></span>
                          <span class="truncate text-sm font-medium {completed ? 'line-through opacity-60' : ''}">
                            跳线 {index + 1} · {wire.net_name || wire.net_id || "未命名网络"}
                          </span>
                        </span>
                        <span class="mt-0.5 block font-mono text-xs text-base-content/55">
                          {holeLabel(wire.from)} → {holeLabel(wire.to)}
                        </span>
                      </button>
                    </li>
                  {/each}
                </ul>
              {:else}
                <div class="py-4 text-center text-sm text-base-content/50">无需添加跳线</div>
              {/if}
            </div>
          </div>
        </div>

        {#if wires.length > 0 && completedWireCount === wires.length}
          <div class="alert alert-success shrink-0 py-2 text-sm" role="status">
            <span class="status status-success"></span>
            <span>所有跳线均已完成</span>
          </div>
        {/if}
      </div>
    </aside>
  </div>
</div>

<style>
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
