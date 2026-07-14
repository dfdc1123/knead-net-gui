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

<div class="mx-auto flex h-full min-h-0 w-full max-w-screen-2xl flex-col gap-4 overflow-hidden p-6">
  <header class="flex shrink-0 items-center justify-between gap-3">
    <h1 class="text-2xl font-bold">装配视图</h1>

    <div class="flex items-center gap-2">
      <div class="join">
        <span class="badge badge-outline join-item h-8">{frame.parts.length} 个元件</span>
        <span class="badge badge-outline join-item h-8">{netCount} 个网络</span>
      </div>
      {#if selected}
        <button class="btn btn-sm btn-ghost" onclick={() => (selected = null)} aria-label="清除高亮">清除</button>
      {/if}
    </div>
  </header>

  {#if selected}
    <div class="alert alert-warning min-h-10 shrink-0 py-2 text-sm">
      <span class="status status-warning" aria-hidden="true"></span>
      <span>
        <span class="badge badge-sm {selected.type === 'component' ? 'badge-primary' : 'badge-secondary'}">{selected.type === "component" ? "元件" : "网络"}</span>
        <strong class="ml-1 font-mono">{selected.label}</strong>
      </span>
    </div>
  {/if}

  <div class="grid min-h-0 flex-1 grid-cols-2 gap-4">
    <section class="card min-h-80 overflow-hidden border border-base-300 bg-base-100 shadow-sm">
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

    <section class="card min-h-80 overflow-hidden border border-base-300 bg-base-100 shadow-sm">
      <div class="card-body min-h-0 gap-2 p-3">
        <div class="flex shrink-0 items-center justify-between px-1">
          <h2 class="card-title text-base">面包板</h2>
          <span class="badge badge-ghost badge-sm">{cols} 列</span>
        </div>
        <div class="min-h-0 flex-1 overflow-auto rounded-box border border-base-300 bg-base-200">
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
    filter: drop-shadow(0 0 5px var(--color-warning)) drop-shadow(0 0 2px var(--color-warning));
  }

  :global(.schematic-host .sch-net-line.is-selected) {
    stroke: var(--color-warning);
    stroke-width: 4;
    filter: drop-shadow(0 0 2px var(--color-warning));
  }
</style>
