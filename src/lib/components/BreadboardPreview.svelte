<script lang="ts">
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
    zoom = 1,
    selected = null,
    completedWireIds = [],
    onSelect = () => {},
  }: {
    preset: BreadboardPreset;
    cols: number;
    frame?: LayoutFrame | null;
    zoom?: number;
    selected?: CircuitSelection | null;
    completedWireIds?: string[];
    onSelect?: (selection: CircuitSelection | null) => void;
  } = $props();

  const pitch = 12;
  const mainRows = [0, 1, 2, 3, 4];

  type Point = { x: number; y: number };

  type PlannedWire = {
    wire: LayoutWire;
    from: Point;
    to: Point;
    horizontal: boolean;
    level: number;
    maxLevel: number;
    direction: -1 | 1;
  };

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

  function intervalsOverlap(left: PlannedWire, right: PlannedWire) {
    const leftStart = Math.min(left.from.x, left.to.x);
    const leftEnd = Math.max(left.from.x, left.to.x);
    const rightStart = Math.min(right.from.x, right.to.x);
    const rightEnd = Math.max(right.from.x, right.to.x);
    // 只在内部相交时算冲突；仅仅共用一个端点不需要把整条线抬高。
    return Math.max(leftStart, rightStart) < Math.min(leftEnd, rightEnd) - 0.01;
  }

  function planWires(wires: LayoutWire[]): PlannedWire[] {
    const plans = wires.map<PlannedWire>((wire) => {
      const from = holePosition(wire.from);
      const to = holePosition(wire.to);
      return {
        wire,
        from,
        to,
        horizontal: Math.abs(from.y - to.y) < 0.01 && Math.abs(from.x - to.x) > 0.01,
        level: 0,
        maxLevel: 0,
        // 上下区域都朝面包板中央弯，整体保持镜像关系。
        direction: (from.y + to.y) / 2 < boardHeight / 2 ? 1 : -1,
      };
    });

    const rows = new Map<string, number[]>();
    for (const [index, plan] of plans.entries()) {
      if (!plan.horizontal) continue;
      const row = plan.from.y.toFixed(2);
      const indices = rows.get(row) ?? [];
      indices.push(index);
      rows.set(row, indices);
    }

    for (const indices of rows.values()) {
      const sorted = [...indices].sort((leftIndex, rightIndex) => {
        const left = plans[leftIndex];
        const right = plans[rightIndex];
        return (
          Math.min(left.from.x, left.to.x) - Math.min(right.from.x, right.to.x) ||
          Math.max(left.from.x, left.to.x) - Math.max(right.from.x, right.to.x)
        );
      });

      // 先找出传递相交的区间组，再在组内分配可复用的拱形高度层。
      const components: number[][] = [];
      let component: number[] = [];
      let componentEnd = Number.NEGATIVE_INFINITY;
      for (const index of sorted) {
        const start = Math.min(plans[index].from.x, plans[index].to.x);
        const end = Math.max(plans[index].from.x, plans[index].to.x);
        if (component.length > 0 && start >= componentEnd - 0.01) {
          components.push(component);
          component = [];
          componentEnd = Number.NEGATIVE_INFINITY;
        }
        component.push(index);
        componentEnd = Math.max(componentEnd, end);
      }
      if (component.length > 0) components.push(component);

      for (const members of components) {
        if (members.length < 2) continue;

        // 短线优先使用靠近板面的层；更长的包含线会占用更高的层。
        const byLength = [...members].sort((leftIndex, rightIndex) => {
          const left = plans[leftIndex];
          const right = plans[rightIndex];
          const leftLength = Math.abs(left.to.x - left.from.x);
          const rightLength = Math.abs(right.to.x - right.from.x);
          return leftLength - rightLength || Math.min(left.from.x, left.to.x) - Math.min(right.from.x, right.to.x);
        });
        const levels: number[][] = [];
        for (const index of byLength) {
          let level = levels.findIndex((membersAtLevel) =>
            membersAtLevel.every((otherIndex) => !intervalsOverlap(plans[index], plans[otherIndex])),
          );
          if (level === -1) {
            level = levels.length;
            levels.push([]);
          }
          levels[level].push(index);
          plans[index].level = level;
        }

        const maxLevel = Math.max(0, levels.length - 1);
        for (const index of members) {
          plans[index].maxLevel = maxLevel;
        }
      }
    }

    return plans;
  }

  let plannedWires = $derived(planWires(frame?.wires ?? []));

  function wirePath(plan: PlannedWire) {
    const { from, to } = plan;
    if (!plan.horizontal) {
      const middleY = (from.y + to.y) / 2;
      return `M ${from.x} ${from.y} C ${from.x} ${middleY}, ${to.x} ${middleY}, ${to.x} ${to.y}`;
    }

    const span = Math.abs(to.x - from.x);
    const baseHeight = Math.min(26, 4 + Math.sqrt(span) * 1.35);
    const availableSpread = 16;
    const maximumStep = 3;
    const levelStep = plan.maxLevel > 0
      ? Math.min(maximumStep, availableSpread / plan.maxLevel)
      : 0;
    const height = baseHeight + plan.level * levelStep;
    const controlY = from.y + plan.direction * height;
    const deltaX = to.x - from.x;

    // 两个控制点保持同高，得到比半圆更扁、更接近真实跳线的拱形。
    return `M ${from.x} ${from.y} C ${from.x + deltaX * 0.22} ${controlY}, ${to.x - deltaX * 0.22} ${controlY}, ${to.x} ${to.y}`;
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

  function selectWire(event: Event, wire: LayoutWire) {
    event.stopPropagation();
    onSelect(
      selected?.type === "wire" && selected.id === wire.id
        ? null
        : {
            type: "wire",
            id: wire.id,
            label: wire.net_name || wire.net_id || wire.id,
            netId: wire.net_id,
          },
    );
  }
</script>

<div
  class="flex min-h-full min-w-full justify-center bg-base-200 p-3 text-base-content"
  data-theme="corporate"
  role="presentation"
  onclick={() => onSelect(null)}
>
  <svg
    width={displayWidth * zoom}
    height={displayHeight * zoom}
    viewBox="0 0 {boardWidth} {boardHeight}"
    role="img"
    aria-label="面包板预览：{preset === 'hole170' ? '170 孔' : preset === 'hole400' ? '400 孔' : '800 规格（默认实际 830 孔）'}"
    class="block max-w-none"
  >
    <rect x="0.8" y="0.8" width={boardWidth - 1.6} height={boardHeight - 1.6} rx="7" fill="var(--color-base-100)" stroke="var(--color-base-300)" stroke-width="1.2" />

    {#if isMini}
      <rect x={xInset - 5} y="78.05" width={boardWidth - 2 * xInset + 10} height="12.1" rx="2" fill="var(--color-base-300)" />
      <path d="M {xInset - 5} 78.6 H {boardWidth - xInset + 5}" stroke="var(--color-base-content)" stroke-opacity="0.3" stroke-width="1" />

      {#each columns as column}
        {#each mainRows as row}
          <g transform="translate({xInset + column * pitch} {18.1 + row * pitch})">
            <circle r="3.6" fill="var(--color-base-100)" stroke="var(--color-base-300)" stroke-width="1" />
            <circle r="1.6" fill="var(--color-base-content)" />
          </g>
          <g transform="translate({xInset + column * pitch} {102.1 + row * pitch})">
            <circle r="3.6" fill="var(--color-base-100)" stroke="var(--color-base-300)" stroke-width="1" />
            <circle r="1.6" fill="var(--color-base-content)" />
          </g>
        {/each}
      {/each}
    {:else}
      <path d="M 1 4 H {boardWidth - 1}" stroke="var(--color-primary)" stroke-width="1.4" opacity="0.9" />
      <path d="M 1 32 H {boardWidth - 1}" stroke="var(--color-error)" stroke-width="1.4" opacity="0.9" />
      <path d="M 1 220 H {boardWidth - 1}" stroke="var(--color-primary)" stroke-width="1.4" opacity="0.9" />
      <path d="M 1 248 H {boardWidth - 1}" stroke="var(--color-error)" stroke-width="1.4" opacity="0.9" />
      <path d="M 1 35 H {boardWidth - 1} M 1 217 H {boardWidth - 1}" stroke="var(--color-base-content)" stroke-opacity="0.3" stroke-width="1" />

      <rect x="1" y="118.7" width={boardWidth - 2} height="12" fill="var(--color-base-300)" />
      <path d="M 1 119.2 H {boardWidth - 1} M 1 130.2 H {boardWidth - 1}" stroke="var(--color-base-content)" stroke-opacity="0.3" stroke-width="1" />

      {#each columns as column}
        {#each mainRows as row}
          <g transform="translate({xInset + column * pitch} {60 + row * pitch})">
            <circle r="3.6" fill="var(--color-base-100)" stroke="var(--color-base-300)" stroke-width="1" />
            <circle r="1.6" fill="var(--color-base-content)" />
          </g>
          <g transform="translate({xInset + column * pitch} {144 + row * pitch})">
            <circle r="3.6" fill="var(--color-base-100)" stroke="var(--color-base-300)" stroke-width="1" />
            <circle r="1.6" fill="var(--color-base-content)" />
          </g>
        {/each}
      {/each}

      {#each powerColumns as column}
        {#each [12, 24, 228, 240] as y}
          <g transform="translate({xInset + railOffset + column * pitch} {y})">
            <circle r="3.6" fill="var(--color-base-100)" stroke="var(--color-base-300)" stroke-width="1" />
            <circle r="1.6" fill="var(--color-base-content)" />
          </g>
        {/each}
      {/each}

      <g font-family="ui-sans-serif, system-ui, sans-serif" font-size="7" font-weight="700" text-anchor="middle">
        <text x="7" y="14.5" fill="var(--color-primary)">−</text>
        <text x="7" y="26.5" fill="var(--color-error)">+</text>
        <text x="7" y="230.5" fill="var(--color-primary)">−</text>
        <text x="7" y="242.5" fill="var(--color-error)">+</text>
      </g>
    {/if}

    {#if frame}
      <g aria-label="布局连线">
        {#each plannedWires as planned (planned.wire.id)}
          {@const wire = planned.wire}
          {@const path = wirePath(planned)}
          {@const completed = completedWireIds.includes(wire.id)}
          <g
            class="cursor-pointer"
            role="button"
            tabindex="0"
            aria-label="选择跳线 {wire.net_name ?? wire.net_id ?? wire.id}"
            onclick={(event) => selectWire(event, wire)}
            onkeydown={(event) => {
              if (event.key === "Enter" || event.key === " ") selectWire(event, wire);
            }}
          >
            <path
              d={path}
              fill="none"
              stroke={wire.kind === "routed" ? wire.color ?? "var(--color-primary)" : "var(--color-neutral)"}
              stroke-width={(selected?.type === "wire" && selected.id === wire.id) || (selected?.type === "net" && selected.id === wire.net_id) ? 5 : completed ? 3 : wire.kind === "routed" ? 2.2 : 1.2}
              stroke-dasharray={wire.kind !== "routed" || !completed ? "5 4" : undefined}
              stroke-linecap="round"
              opacity={selected ? ((selected.type === "wire" && selected.id === wire.id) || (selected.type === "net" && selected.id === wire.net_id) ? 1 : 0.14) : completed ? 0.95 : 0.38}
              pointer-events="none"
            />
            <!-- 用更宽的透明路径承接鼠标事件，细线仍然容易选中。 -->
            <path
              d={path}
              fill="none"
              stroke="transparent"
              stroke-width="10"
              stroke-linecap="round"
              pointer-events="stroke"
            />
          </g>
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
            onclick={(event) => selectComponent(event, part.reference)}
            onkeydown={(event) => {
              if (event.key === "Enter" || event.key === " ") selectComponent(event, part.reference);
            }}
          >
          {#if part.kind === "axial" && part.pins.length >= 2}
            {@const first = holePosition(part.pins[0].hole)}
            {@const last = holePosition(part.pins[part.pins.length - 1].hole)}
            <line x1={first.x} y1={first.y} x2={last.x} y2={last.y} stroke="var(--color-neutral)" stroke-width="1.4" />
            <rect
              x={bounds.cx - Math.min(14, Math.max(7, bounds.width / 4))}
              y={bounds.cy - 4.5}
              width={Math.min(28, Math.max(14, bounds.width / 2))}
              height="9"
              rx="3"
              fill="var(--color-warning)"
              stroke="var(--color-neutral)"
              stroke-width="1"
            />
          {:else}
            <rect
              x={bounds.x}
              y={bounds.y}
              width={bounds.width}
              height={bounds.height}
              rx={part.kind === "ic" ? 2 : 4}
              fill={part.kind === "ic" ? "var(--color-neutral)" : "var(--color-base-200)"}
              stroke={part.kind === "ic" ? "var(--color-base-content)" : "var(--color-neutral)"}
              stroke-width="1.2"
            />
          {/if}

          {#each part.pins as pin}
            {@const point = holePosition(pin.hole)}
            <circle
              cx={point.x}
              cy={point.y}
              r={selected?.type === "net" && selected.id === pin.net_id ? 4 : 2.4}
              fill={selected?.type === "net" && selected.id === pin.net_id ? "var(--color-warning)" : "var(--color-base-100)"}
              stroke="var(--color-neutral)"
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
            fill={part.kind === "ic" ? "var(--color-neutral-content)" : "var(--color-base-content)"}
            pointer-events="none"
          >{part.reference}</text>
          <title>{part.reference}{part.value ? ` · ${part.value}` : ""}</title>
          </g>
        {/each}
      </g>
    {/if}
  </svg>
</div>
