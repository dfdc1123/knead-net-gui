<script lang="ts">
  import type {
    BreadboardHole,
    BreadboardPreset,
    CircuitSelection,
    LayoutFrame,
    LayoutPart,
  } from "$lib/layout";

  let {
    preset,
    cols,
    frame,
    selected = null,
    onSelect = () => {},
  }: {
    preset: BreadboardPreset;
    cols: number;
    frame?: LayoutFrame | null;
    selected?: CircuitSelection | null;
    onSelect?: (selection: CircuitSelection | null) => void;
  } = $props();

  const pitch = 12;
  const mainRows = [0, 1, 2, 3, 4];

  function range(length: number) {
    return Array.from({ length }, (_, index) => index);
  }

  function railColumns(kind: BreadboardPreset, columnCount: number) {
    const margin = kind === "hole800" ? 2 : 0;
    const result: number[] = [];
    for (let start = margin; start < columnCount - margin; start += 6) {
      for (let offset = 0; offset < 5 && start + offset < columnCount - margin; offset += 1) {
        result.push(start + offset);
      }
    }
    return result;
  }

  let safeCols = $derived(Math.max(1, Math.trunc(Number(cols) || 1)));
  let isMini = $derived(preset === "hole170");
  let xInset = $derived(isMini ? 12.2 : 18.2);
  let columns = $derived(range(safeCols));
  let powerColumns = $derived(railColumns(preset, safeCols));

  // 400 孔电源轨的 5 组孔占 0..28 节拍，主区占 0..29 节拍；
  // 横移半个孔距后两者中心重合。这只是绘图坐标，算法仍使用整数列。
  let railOffset = $derived(preset === "hole400" ? pitch * 0.5 : 0);
  let contentWidth = $derived(
    Math.max(
      (safeCols - 1) * pitch,
      powerColumns.length > 0 ? powerColumns[powerColumns.length - 1] * pitch + railOffset : 0,
    ),
  );
  let boardWidth = $derived(xInset * 2 + contentWidth);
  let boardHeight = $derived(isMini ? 168.2 : 252);
  let displayWidth = $derived(Math.max(isMini ? 420 : 440, boardWidth));
  let displayHeight = $derived((displayWidth / boardWidth) * boardHeight);

  function holePosition(hole: BreadboardHole) {
    const x = xInset + hole.col * pitch + (hole.region.startsWith("rail") ? railOffset : 0);
    if (isMini) {
      return {
        x,
        y: (hole.region === "main-bottom" ? 102.1 : 18.1) + hole.row * pitch,
      };
    }

    const bases: Record<BreadboardHole["region"], number> = {
      "rail-top": 12,
      "main-top": 60,
      "main-bottom": 144,
      "rail-bottom": 228,
    };
    return { x, y: bases[hole.region] + hole.row * pitch };
  }

  function partBounds(part: LayoutPart) {
    const points = part.pins.map((pin) => holePosition(pin.hole));
    if (points.length === 0) {
      return {
        x: boardWidth / 2 - 10,
        y: boardHeight / 2 - 6,
        width: 20,
        height: 12,
        cx: boardWidth / 2,
        cy: boardHeight / 2,
      };
    }
    const xs = points.map((point) => point.x);
    const ys = points.map((point) => point.y);
    const minX = Math.min(...xs);
    const maxX = Math.max(...xs);
    const minY = Math.min(...ys);
    const maxY = Math.max(...ys);
    return {
      x: minX - 5,
      y: minY - 5,
      width: Math.max(10, maxX - minX + 10),
      height: Math.max(10, maxY - minY + 10),
      cx: (minX + maxX) / 2,
      cy: (minY + maxY) / 2,
    };
  }

  function selectComponent(event: Event, reference: string) {
    event.stopPropagation();
    onSelect(
      selected?.type === "component" && selected.id === reference
        ? null
        : { type: "component", id: reference, label: reference },
    );
  }

  function selectNet(event: Event, id?: string, label?: string) {
    event.stopPropagation();
    if (!id) return;
    onSelect(
      selected?.type === "net" && selected.id === id
        ? null
        : { type: "net", id, label: label || id },
    );
  }
</script>

<div class="flex min-w-full justify-center p-3" role="presentation" onclick={() => onSelect(null)}>
  <svg
    width={displayWidth}
    height={displayHeight}
    viewBox="0 0 {boardWidth} {boardHeight}"
    role="img"
    aria-label="面包板预览：{preset === 'hole170' ? '170 孔' : preset === 'hole400' ? '400 孔' : '800 规格（默认实际 830 孔）'}"
    class="block max-w-none drop-shadow-md"
  >
    <defs>
      <linearGradient id="board-surface" x1="0" y1="0" x2="0" y2="1">
        <stop offset="0" stop-color="#fafafa" />
        <stop offset="0.48" stop-color="#e8e8e6" />
        <stop offset="1" stop-color="#d4d4d1" />
      </linearGradient>
      <radialGradient id="socket-rim" cx="42%" cy="36%" r="65%">
        <stop offset="0" stop-color="#ffffff" />
        <stop offset="0.55" stop-color="#d5d5d2" />
        <stop offset="1" stop-color="#a8a8a5" />
      </radialGradient>
      <filter id="inset-shadow" x="-20%" y="-20%" width="140%" height="140%">
        <feDropShadow dx="0" dy="0.7" stdDeviation="0.45" flood-color="#000" flood-opacity="0.45" />
      </filter>
      <filter id="selection-glow" x="-40%" y="-40%" width="180%" height="180%">
        <feDropShadow dx="0" dy="0" stdDeviation="2.5" flood-color="#f59e0b" flood-opacity="1" />
      </filter>
    </defs>

    <rect x="0.8" y="0.8" width={boardWidth - 1.6} height={boardHeight - 1.6} rx="7" fill="url(#board-surface)" stroke="#b8b8b5" stroke-width="1.6" />

    {#if isMini}
      <rect x={xInset - 5} y="78.05" width={boardWidth - 2 * xInset + 10} height="12.1" rx="2" fill="#c8c8c5" />
      <path d="M {xInset - 5} 78.6 H {boardWidth - xInset + 5}" stroke="#b4b4b1" stroke-width="1" />

      {#each columns as column}
        {#each mainRows as row}
          <g transform="translate({xInset + column * pitch} {18.1 + row * pitch})">
            <circle r="4" fill="url(#socket-rim)" />
            <circle r="1.8" fill="#343434" filter="url(#inset-shadow)" />
          </g>
          <g transform="translate({xInset + column * pitch} {102.1 + row * pitch})">
            <circle r="4" fill="url(#socket-rim)" />
            <circle r="1.8" fill="#343434" filter="url(#inset-shadow)" />
          </g>
        {/each}
      {/each}
    {:else}
      <path d="M 1 4 H {boardWidth - 1}" stroke="#2563eb" stroke-width="1.4" opacity="0.9" />
      <path d="M 1 32 H {boardWidth - 1}" stroke="#dc2626" stroke-width="1.4" opacity="0.9" />
      <path d="M 1 220 H {boardWidth - 1}" stroke="#2563eb" stroke-width="1.4" opacity="0.9" />
      <path d="M 1 248 H {boardWidth - 1}" stroke="#dc2626" stroke-width="1.4" opacity="0.9" />
      <path d="M 1 35 H {boardWidth - 1} M 1 217 H {boardWidth - 1}" stroke="#aaa" stroke-width="1" />

      <rect x="1" y="118.7" width={boardWidth - 2} height="12" fill="#c8c8c5" />
      <path d="M 1 119.2 H {boardWidth - 1} M 1 130.2 H {boardWidth - 1}" stroke="#b0b0ad" stroke-width="1" />

      {#each columns as column}
        {#each mainRows as row}
          <g transform="translate({xInset + column * pitch} {60 + row * pitch})">
            <circle r="4" fill="url(#socket-rim)" />
            <circle r="1.8" fill="#343434" filter="url(#inset-shadow)" />
          </g>
          <g transform="translate({xInset + column * pitch} {144 + row * pitch})">
            <circle r="4" fill="url(#socket-rim)" />
            <circle r="1.8" fill="#343434" filter="url(#inset-shadow)" />
          </g>
        {/each}
      {/each}

      {#each powerColumns as column}
        {#each [12, 24, 228, 240] as y}
          <g transform="translate({xInset + railOffset + column * pitch} {y})">
            <circle r="4" fill="url(#socket-rim)" />
            <circle r="1.8" fill="#343434" filter="url(#inset-shadow)" />
          </g>
        {/each}
      {/each}

      <g font-family="ui-sans-serif, system-ui, sans-serif" font-size="7" font-weight="700" text-anchor="middle">
        <text x="7" y="14.5" fill="#2563eb">−</text>
        <text x="7" y="26.5" fill="#dc2626">+</text>
        <text x="7" y="230.5" fill="#2563eb">−</text>
        <text x="7" y="242.5" fill="#dc2626">+</text>
      </g>
    {/if}

    {#if frame}
      <g aria-label="布局连线">
        {#each frame.wires ?? [] as wire (wire.id)}
          {@const from = holePosition(wire.from)}
          {@const to = holePosition(wire.to)}
          <path
            d="M {from.x} {from.y} C {from.x} {(from.y + to.y) / 2}, {to.x} {(from.y + to.y) / 2}, {to.x} {to.y}"
            fill="none"
            stroke={wire.color ?? (wire.kind === "routed" ? "#2563eb" : "#64748b")}
            stroke-width={selected?.type === "net" && selected.id === wire.net_id ? 5 : wire.kind === "routed" ? 2.5 : 1.2}
            stroke-dasharray={wire.kind === "routed" ? undefined : "4 3"}
            stroke-linecap="round"
            opacity={selected ? (selected.type === "net" && selected.id === wire.net_id ? 1 : 0.18) : wire.kind === "routed" ? 0.9 : 0.65}
            class="cursor-pointer transition-all"
            role="button"
            tabindex="0"
            aria-label="选择网络 {wire.net_name ?? wire.net_id ?? wire.id}"
            onclick={(event) => selectNet(event, wire.net_id, wire.net_name)}
            onkeydown={(event) => {
              if (event.key === "Enter" || event.key === " ") selectNet(event, wire.net_id, wire.net_name);
            }}
          />
        {/each}
      </g>

      <g aria-label="布局元件">
        {#each frame.parts as part (part.id)}
          {@const bounds = partBounds(part)}
          <g
            class="cursor-pointer transition-opacity"
            role="button"
            tabindex="0"
            aria-label="选择元件 {part.reference}"
            opacity={selected?.type === "component" ? (selected.id === part.reference ? 1 : 0.25) : 1}
            filter={selected?.type === "component" && selected.id === part.reference ? "url(#selection-glow)" : undefined}
            onclick={(event) => selectComponent(event, part.reference)}
            onkeydown={(event) => {
              if (event.key === "Enter" || event.key === " ") selectComponent(event, part.reference);
            }}
          >
          {#if part.kind === "axial" && part.pins.length >= 2}
            {@const first = holePosition(part.pins[0].hole)}
            {@const last = holePosition(part.pins[part.pins.length - 1].hole)}
            <line x1={first.x} y1={first.y} x2={last.x} y2={last.y} stroke="#52525b" stroke-width="1.4" />
            <rect
              x={bounds.cx - Math.min(14, Math.max(7, bounds.width / 4))}
              y={bounds.cy - 4.5}
              width={Math.min(28, Math.max(14, bounds.width / 2))}
              height="9"
              rx="3"
              fill={part.color ?? "#d6b27a"}
              stroke="#713f12"
              stroke-width="1"
            />
          {:else}
            <rect
              x={bounds.x}
              y={bounds.y}
              width={bounds.width}
              height={bounds.height}
              rx={part.kind === "ic" ? 2 : 4}
              fill={part.color ?? (part.kind === "ic" ? "#27272a" : "#e4e4e7")}
              stroke={part.kind === "ic" ? "#09090b" : "#52525b"}
              stroke-width="1.2"
            />
          {/if}

          {#each part.pins as pin}
            {@const point = holePosition(pin.hole)}
            <circle
              cx={point.x}
              cy={point.y}
              r={selected?.type === "net" && selected.id === pin.net_id ? 4 : 2.4}
              fill={selected?.type === "net" && selected.id === pin.net_id ? "#f59e0b" : "#d4d4d8"}
              stroke="#3f3f46"
              stroke-width="0.8"
            >
              <title>{part.reference} pin {pin.number ?? "?"}{pin.name ? ` · ${pin.name}` : ""}</title>
            </circle>
          {/each}
          <text
            x={bounds.cx}
            y={bounds.cy + 2.3}
            text-anchor="middle"
            font-family="ui-sans-serif, system-ui, sans-serif"
            font-size="6.5"
            font-weight="700"
            fill={part.kind === "ic" ? "#fafafa" : "#18181b"}
            pointer-events="none"
          >{part.reference}</text>
          <title>{part.reference}{part.value ? ` · ${part.value}` : ""}</title>
          </g>
        {/each}
      </g>
    {/if}
  </svg>
</div>
