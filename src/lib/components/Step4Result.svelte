<script lang="ts">
  import BreadboardPreview from "./BreadboardPreview.svelte";
  import type {
    BreadboardPreset,
    CircuitSelection,
    LayoutFrame,
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
  let schematicHost = $state<HTMLDivElement>();

  let netCount = $derived(new Set((frame.wires ?? []).map((wire) => wire.net_id).filter(Boolean)).size);

  function choose(next: CircuitSelection | null) {
    selected =
      next && selected?.type === next.type && selected.id === next.id
        ? null
        : next;
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
</script>

<div class="flex h-full min-h-0 flex-col gap-3 overflow-hidden p-4 lg:p-6">
  <div class="flex shrink-0 flex-wrap items-center justify-between gap-3">
    <div>
      <h2 class="text-lg font-bold">交互式装配视图</h2>
      <p class="text-xs text-base-content/55">点击任一侧的元器件或网络，另一侧会同步高亮。</p>
    </div>

    <div class="flex items-center gap-2">
      <div class="join">
        <span class="badge badge-outline join-item h-8">{frame.parts.length} 个元件</span>
        <span class="badge badge-outline join-item h-8">{netCount} 个网络</span>
      </div>
      {#if selected}
        <button class="btn btn-sm btn-ghost" onclick={() => (selected = null)} aria-label="清除高亮">清除</button>
      {/if}
    </div>
  </div>

  <div class="alert min-h-11 shrink-0 py-2 text-sm {selected ? 'alert-warning' : 'alert-info'}">
    <span class="text-lg" aria-hidden="true">{selected?.type === "component" ? "◆" : selected?.type === "net" ? "━" : "↔"}</span>
    {#if selected}
      <span>
        已选中 <span class="badge badge-sm {selected.type === 'component' ? 'badge-primary' : 'badge-secondary'}">{selected.type === "component" ? "元件" : "网络"}</span>
        <strong class="ml-1 font-mono">{selected.label}</strong>
      </span>
    {:else}
      <span>在原理图或面包板上点击开始查看对应关系</span>
    {/if}
  </div>

  <div class="grid min-h-0 flex-1 grid-cols-1 gap-3 lg:grid-cols-2">
    <section class="card min-h-80 overflow-hidden border border-base-300 bg-base-200 shadow-sm">
      <div class="card-body min-h-0 gap-2 p-3">
        <div class="flex shrink-0 items-center justify-between px-1">
          <h3 class="card-title text-sm">原理图</h3>
          <span class="badge badge-ghost badge-sm">SCH</span>
        </div>
        {#if schematicSvg}
          <div
            class="schematic-host min-h-0 flex-1 overflow-auto rounded-box bg-white p-2"
            bind:this={schematicHost}
            onclick={handleSchematicClick}
            role="presentation"
          >
            {@html schematicSvg}
          </div>
        {:else}
          <div class="grid min-h-0 flex-1 place-items-center rounded-box bg-base-100 p-6 text-center text-sm text-base-content/50">
            当前项目没有可显示的原理图；仍可查看右侧最终布局。
          </div>
        {/if}
      </div>
    </section>

    <section class="card min-h-80 overflow-hidden border border-base-300 bg-base-200 shadow-sm">
      <div class="card-body min-h-0 gap-2 p-3">
        <div class="flex shrink-0 items-center justify-between px-1">
          <h3 class="card-title text-sm">面包板</h3>
          <span class="badge badge-ghost badge-sm">{cols} 列</span>
        </div>
        <div class="min-h-0 flex-1 overflow-auto rounded-box bg-base-100">
          <BreadboardPreview
            {preset}
            {cols}
            {frame}
            {selected}
            onSelect={choose}
          />
        </div>
      </div>
    </section>
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
    filter: drop-shadow(0 0 5px #f59e0b) drop-shadow(0 0 2px #f59e0b);
  }

  :global(.schematic-host .sch-net-line.is-selected) {
    stroke: #f59e0b;
    stroke-width: 4;
    filter: drop-shadow(0 0 2px #f59e0b);
  }
</style>
