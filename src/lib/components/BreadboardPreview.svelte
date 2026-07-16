<script lang="ts">
  import type {
    BreadboardHole,
    BreadboardPreset,
    CircuitSelection,
    LayoutFrame,
    LayoutPart,
    LayoutPin,
    LayoutWire,
  } from "$lib/layout";
  import { ui } from "$lib/i18n";

  let {
    preset,
    cols,
    upperHalfOnly = false,
    frame,
    zoom = 1,
    fitWidth = 0,
    fitHeight = 0,
    panCanvas = true,
    solidWires = false,
    selected = null,
    completedWireIds = [],
    tieNegativeRails = true,
    tiePositiveRails = true,
    onSelect = () => {},
  }: {
    preset: BreadboardPreset;
    cols: number;
    upperHalfOnly?: boolean;
    frame?: LayoutFrame | null;
    zoom?: number;
    fitWidth?: number;
    fitHeight?: number;
    panCanvas?: boolean;
    solidWires?: boolean;
    selected?: CircuitSelection | null;
    completedWireIds?: string[];
    tieNegativeRails?: boolean;
    tiePositiveRails?: boolean;
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

  function presetRailTies(
    kind: BreadboardPreset,
    columnCount: number,
    includeNegative: boolean,
    includePositive: boolean,
  ): LayoutWire[] {
    if (kind === "hole170") return [];
    const availableColumns = railColumns(kind, columnCount);
    const col = availableColumns[availableColumns.length - 1];
    if (col === undefined) return [];
    const ties: LayoutWire[] = [
      {
        id: "rail-tie:preset:negative:top-bottom",
        from: { region: "rail-top", col, row: 0 },
        to: { region: "rail-bottom", col, row: 0 },
        color: "#2f6fbd",
        kind: "rail-tie",
        net_id: "power-rail-negative",
        net_name: "negative power-rail tie",
      },
      {
        id: "rail-tie:preset:positive:top-bottom",
        from: { region: "rail-top", col, row: 1 },
        to: { region: "rail-bottom", col, row: 1 },
        color: "#c83434",
        kind: "rail-tie",
        net_id: "power-rail-positive",
        net_name: "positive power-rail tie",
      },
    ];
    return ties.filter((wire) => wire.id.includes(":negative:") ? includeNegative : includePositive);
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
  let boardHeight = $derived(isMini ? (upperHalfOnly ? 84.2 : 168.2) : (upperHalfOnly ? 132 : 252));
  let displayWidth = $derived(Math.max(isMini ? 420 : 440, boardWidth));
  let displayHeight = $derived((displayWidth / boardWidth) * boardHeight);
  let renderedZoom = $derived.by(() => {
    if (fitWidth <= 0 || fitHeight <= 0) return zoom;
    const availableWidth = Math.max(1, fitWidth - 24);
    const availableHeight = Math.max(1, fitHeight - 24);
    const fitScale = Math.min(availableWidth / displayWidth, availableHeight / displayHeight);
    return zoom * fitScale;
  });
  let hasPanPadding = $derived(panCanvas && zoom > 1);

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

  let visibleWires = $derived.by(() => {
    const wires = frame?.wires ?? [];
    return frame
      ? wires
      : upperHalfOnly
        ? wires
        : [...wires, ...presetRailTies(preset, safeCols, tieNegativeRails, tiePositiveRails)];
  });
  let plannedWires = $derived(planWires(visibleWires));

  function wirePath(plan: PlannedWire) {
    const { from, to } = plan;
    if (plan.wire.kind === "rail-tie") {
      const bulge = plan.wire.id.includes(":positive:") ? 16 : 9;
      const middleY = (from.y + to.y) / 2;
      return `M ${from.x} ${from.y} C ${from.x + bulge} ${middleY}, ${to.x + bulge} ${middleY}, ${to.x} ${to.y}`;
    }
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

  function pinByNumber(part: LayoutPart, number: string) {
    return part.pins.find((pin) => pin.number === number);
  }

  function normalizedPinName(pin: LayoutPin) {
    return pin.name?.trim().toUpperCase();
  }

  function pinLabelText(pin: LayoutPin) {
    const name = pin.name?.trim();
    const detail = [name, pin.pin_type?.trim(), pin.pin_shape?.trim()].filter(Boolean).join(" · ");
    return `${pin.number || "?"}${detail ? ` · ${detail}` : ""}`;
  }

  function pinCalloutText(pin: LayoutPin) {
    const name = pin.name?.trim();
    const detail = [name, pin.pin_type?.trim()].filter(Boolean).join(" · ");
    return `${pin.number || "?"}${detail ? ` · ${detail}` : ""}`;
  }

  function axialGeometry(part: LayoutPart) {
    if (part.pins.length < 2) return null;
    const first = holePosition(part.pins[0].hole);
    const last = holePosition(part.pins[part.pins.length - 1].hole);
    const dx = last.x - first.x;
    const dy = last.y - first.y;
    const distance = Math.hypot(dx, dy);
    if (distance < 0.01) return null;
    return {
      first,
      last,
      cx: (first.x + last.x) / 2,
      cy: (first.y + last.y) / 2,
      angle: Math.atan2(dy, dx) * 180 / Math.PI,
      bodyWidth: Math.min(28, Math.max(14, distance / 2)),
      normalX: -dy / distance,
      normalY: dx / distance,
    };
  }

  function cathodeBandX(part: LayoutPart, bodyWidth: number) {
    const cathodeIndex = part.pins.findIndex((pin) => normalizedPinName(pin) === "K");
    if (cathodeIndex < 0) return null;
    return (cathodeIndex === 0 ? -1 : 1) * bodyWidth * 0.3;
  }

  function polarityLabelPoint(part: LayoutPart, pin: LayoutPin) {
    const geometry = axialGeometry(part);
    const point = holePosition(pin.hole);
    if (!geometry) return point;
    return {
      x: point.x + geometry.normalX * 7,
      y: point.y + geometry.normalY * 7,
    };
  }

  function pinOneDot(part: LayoutPart) {
    const pin = pinByNumber(part, "1");
    if (!pin) return null;
    const point = holePosition(pin.hole);
    const bounds = partBounds(part);
    const dx = bounds.cx - point.x;
    const dy = bounds.cy - point.y;
    const distance = Math.hypot(dx, dy);
    if (distance < 0.01) return point;
    return {
      x: point.x + dx / distance * 5,
      y: point.y + dy / distance * 5,
    };
  }

  function dipNotch(part: LayoutPart) {
    const pin1 = pinByNumber(part, "1");
    if (!pin1) return null;
    const numericPins = part.pins
      .map((pin) => ({ pin, number: Number.parseInt(pin.number ?? "", 10) }))
      .filter(({ number }) => Number.isFinite(number));
    const lastPin = numericPins.length > 0
      ? numericPins.reduce(
          (last, candidate) => candidate.number > last.number ? candidate : last,
          numericPins[0],
        ).pin
      : pin1;
    const firstPoint = holePosition(pin1.hole);
    const lastPoint = holePosition(lastPin.hole);
    const target = {
      x: (firstPoint.x + lastPoint.x) / 2,
      y: (firstPoint.y + lastPoint.y) / 2,
    };
    const bounds = partBounds(part);
    const dx = target.x - bounds.cx;
    const dy = target.y - bounds.cy;

    if (Math.abs(dx) > Math.abs(dy)) {
      return {
        side: dx < 0 ? "left" as const : "right" as const,
        center: target.y,
      };
    }

    return {
      side: dy < 0 ? "top" as const : "bottom" as const,
      center: target.x,
    };
  }

  function dipBodyPath(part: LayoutPart) {
    const bounds = partBounds(part);
    const left = bounds.x;
    const right = bounds.x + bounds.width;
    const top = bounds.y;
    const bottom = bounds.y + bounds.height;
    const corner = 2;
    const notchRadius = 3.4;
    const notch = dipNotch(part);
    const horizontalCenter = Math.max(
      left + corner + notchRadius,
      Math.min(right - corner - notchRadius, notch?.center ?? bounds.cx),
    );
    const verticalCenter = Math.max(
      top + corner + notchRadius,
      Math.min(bottom - corner - notchRadius, notch?.center ?? bounds.cy),
    );

    const topEdge = notch?.side === "top"
      ? `H ${horizontalCenter - notchRadius} A ${notchRadius} ${notchRadius} 0 0 0 ${horizontalCenter + notchRadius} ${top}`
      : "";
    const rightEdge = notch?.side === "right"
      ? `V ${verticalCenter - notchRadius} A ${notchRadius} ${notchRadius} 0 0 0 ${right} ${verticalCenter + notchRadius}`
      : "";
    const bottomEdge = notch?.side === "bottom"
      ? `H ${horizontalCenter + notchRadius} A ${notchRadius} ${notchRadius} 0 0 0 ${horizontalCenter - notchRadius} ${bottom}`
      : "";
    const leftEdge = notch?.side === "left"
      ? `V ${verticalCenter + notchRadius} A ${notchRadius} ${notchRadius} 0 0 0 ${left} ${verticalCenter - notchRadius}`
      : "";

    return [
      `M ${left + corner} ${top}`,
      topEdge,
      `H ${right - corner} Q ${right} ${top} ${right} ${top + corner}`,
      rightEdge,
      `V ${bottom - corner} Q ${right} ${bottom} ${right - corner} ${bottom}`,
      bottomEdge,
      `H ${left + corner} Q ${left} ${bottom} ${left} ${bottom - corner}`,
      leftEdge,
      `V ${top + corner} Q ${left} ${top} ${left + corner} ${top}`,
      "Z",
    ].join(" ");
  }

  function selectedPinLabel(part: LayoutPart, pin: LayoutPin) {
    const point = holePosition(pin.hole);
    const bounds = partBounds(part);
    const points = part.pins.map((candidate) => holePosition(candidate.hole));
    const spanX = Math.max(...points.map(({ x }) => x)) - Math.min(...points.map(({ x }) => x));
    const spanY = Math.max(...points.map(({ y }) => y)) - Math.min(...points.map(({ y }) => y));
    // 符号引脚形状（如 line / inverted）只影响原理图画法，装配标注不展示。
    const text = pinCalloutText(pin);
    const width = Math.min(72, Math.max(14, text.length * 3.6 + 7));
    const height = 10;
    let x = point.x;
    let y = point.y;

    if (spanY < 0.1) {
      const ordered = part.pins
        .map((candidate, index) => ({ index, point: holePosition(candidate.hole) }))
        .sort((left, right) => left.point.x - right.point.x);
      const order = ordered.findIndex(({ index }) => part.pins[index] === pin);
      if (order === 0) {
        x -= width / 2 + 8;
      } else if (order === ordered.length - 1) {
        x += width / 2 + 8;
      } else {
        const direction = bounds.cy <= boardHeight / 2 ? -1 : 1;
        y += direction * (13 + (order - 1) * 11);
      }
    } else if (spanX < 0.1) {
      const ordered = part.pins
        .map((candidate, index) => ({ index, point: holePosition(candidate.hole) }))
        .sort((left, right) => left.point.y - right.point.y);
      const order = ordered.findIndex(({ index }) => part.pins[index] === pin);
      if (order === 0) {
        y -= 13;
      } else if (order === ordered.length - 1) {
        y += 13;
      } else {
        const direction = bounds.cx <= boardWidth / 2 ? -1 : 1;
        x += direction * (width / 2 + 8 + (order - 1) * 8);
      }
    } else {
      const dx = point.x - bounds.cx;
      const dy = point.y - bounds.cy;
      if (Math.abs(dx) >= Math.abs(dy)) {
        x += (dx < 0 ? -1 : 1) * (width / 2 + 8);
      } else {
        y += (dy < 0 ? -1 : 1) * 13;
      }
    }

    return { point, text, width, height, x, y };
  }

  type PinCallout = ReturnType<typeof selectedPinLabel> & {
    pin: LayoutPin;
    baseX: number;
    baseY: number;
    lane: number;
    stepX: number;
    stepY: number;
  };

  function calloutsOverlap(
    left: Pick<PinCallout, "x" | "y" | "width" | "height">,
    right: Pick<PinCallout, "x" | "y" | "width" | "height">,
    gap = 0,
  ) {
    return (
      Math.abs(left.x - right.x) < (left.width + right.width) / 2 + gap &&
      Math.abs(left.y - right.y) < (left.height + right.height) / 2 + gap
    );
  }

  function calloutStep(label: ReturnType<typeof selectedPinLabel>) {
    const dx = label.x - label.point.x;
    const dy = label.y - label.point.y;
    if (Math.abs(dx) >= Math.abs(dy) && Math.abs(dx) > 0.1) {
      return { x: Math.sign(dx) * pitch, y: 0 };
    }
    return { x: 0, y: (Math.sign(dy) || -1) * pitch };
  }

  function moveCalloutToLane(callout: PinCallout, lane: number) {
    callout.lane = lane;
    callout.x = callout.baseX + callout.stepX * lane;
    callout.y = callout.baseY + callout.stepY * lane;
  }

  function planSelectedPinLabels(part: LayoutPart) {
    const planned: PinCallout[] = [];

    for (const pin of part.pins) {
      const base = selectedPinLabel(part, pin);
      const step = calloutStep(base);
      const callout: PinCallout = {
        ...base,
        pin,
        baseX: base.x,
        baseY: base.y,
        lane: 0,
        stepX: step.x,
        stepY: step.y,
      };

      // 先在初始位置上做冲突图着色。一个小的安全间距也算冲突，
      // 这样相邻 DIP 引脚即使只剩很窄的缝，也会落到交错的通道中。
      const conflictingLanes = new Set(
        planned
          .filter((other) => calloutsOverlap(
            callout,
            { ...other, x: other.baseX, y: other.baseY },
            3,
          ))
          .map((other) => other.lane),
      );
      let lane = 0;
      while (conflictingLanes.has(lane)) lane += 1;
      moveCalloutToLane(callout, lane);

      // 文本宽度不同可能让交错后的框仍然相交；继续沿注释线方向
      // 每次外扩一个面包板孔距，直到获得实际间隙。
      let attempts = 0;
      while (planned.some((other) => calloutsOverlap(callout, other, 1)) && attempts < 8) {
        lane += 1;
        moveCalloutToLane(callout, lane);
        attempts += 1;
      }
      planned.push(callout);
    }

    return planned;
  }

  function calloutLeaderEnd(callout: PinCallout) {
    const dx = callout.point.x - callout.x;
    const dy = callout.point.y - callout.y;
    const halfWidth = callout.width / 2;
    const halfHeight = callout.height / 2;
    const edgeScale = 1 / Math.max(Math.abs(dx) / halfWidth, Math.abs(dy) / halfHeight);

    if (!Number.isFinite(edgeScale) || edgeScale >= 1) {
      return { x: callout.x, y: callout.y };
    }

    return {
      x: callout.x + dx * edgeScale,
      y: callout.y + dy * edgeScale,
    };
  }

  function hasDarkBody(part: LayoutPart) {
    return part.package === "dip" || part.device === "diode";
  }

  type LabelRect = { x: number; y: number; width: number; height: number };

  function labelRect(part: LayoutPart, point: Point): LabelRect {
    const width = Math.max(10, part.reference.length * 4.2 + 2);
    const height = 8;
    return {
      x: point.x - width / 2,
      y: point.y - height / 2,
      width,
      height,
    };
  }

  function overlapArea(left: LabelRect, right: LabelRect) {
    const width = Math.max(0, Math.min(left.x + left.width, right.x + right.width) - Math.max(left.x, right.x));
    const height = Math.max(0, Math.min(left.y + left.height, right.y + right.height) - Math.max(left.y, right.y));
    return width * height;
  }

  function planPartLabels(parts: LayoutPart[]) {
    const bounds = parts.map(partBounds);
    const pins = parts.flatMap((part) => part.pins.map((pin) => holePosition(pin.hole)));
    const placedLabels: LabelRect[] = [];
    const result = new Map<string, Point>();

    for (const [partIndex, part] of parts.entries()) {
      const ownBounds = bounds[partIndex];
      const horizontalOffset = Math.max(8, part.reference.length * 2.1 + 4);
      const above: Point = { x: ownBounds.cx, y: ownBounds.y - 6 };
      const below: Point = { x: ownBounds.cx, y: ownBounds.y + ownBounds.height + 6 };
      const left: Point = { x: ownBounds.x - horizontalOffset, y: ownBounds.cy };
      const right: Point = { x: ownBounds.x + ownBounds.width + horizontalOffset, y: ownBounds.cy };
      const outwardFirst = ownBounds.cy <= boardHeight / 2 ? [above, below] : [below, above];
      const candidates: Point[] = [
        { x: ownBounds.cx, y: ownBounds.cy },
        ...outwardFirst,
        left,
        right,
        { x: left.x, y: outwardFirst[0].y },
        { x: right.x, y: outwardFirst[0].y },
        { x: left.x, y: outwardFirst[1].y },
        { x: right.x, y: outwardFirst[1].y },
      ];

      let bestPoint = candidates[0];
      let bestRect = labelRect(part, bestPoint);
      let bestPenalty = Number.POSITIVE_INFINITY;

      for (const [candidateIndex, candidate] of candidates.entries()) {
        const rect = labelRect(part, candidate);
        let penalty = candidateIndex * 0.01;

        const overflowLeft = Math.max(0, 2 - rect.x);
        const overflowTop = Math.max(0, 2 - rect.y);
        const overflowRight = Math.max(0, rect.x + rect.width - (boardWidth - 2));
        const overflowBottom = Math.max(0, rect.y + rect.height - (boardHeight - 2));
        penalty += (overflowLeft + overflowTop + overflowRight + overflowBottom) * 1000;

        for (const pin of pins) {
          if (
            pin.x >= rect.x - 3 &&
            pin.x <= rect.x + rect.width + 3 &&
            pin.y >= rect.y - 3 &&
            pin.y <= rect.y + rect.height + 3
          ) penalty += 1000;
        }

        for (const [otherIndex, otherBounds] of bounds.entries()) {
          if (otherIndex === partIndex) continue;
          penalty += overlapArea(rect, otherBounds) * 50;
        }
        for (const placed of placedLabels) {
          penalty += overlapArea(rect, placed) * 100;
        }

        if (penalty < bestPenalty) {
          bestPoint = candidate;
          bestRect = rect;
          bestPenalty = penalty;
        }
      }

      result.set(part.id, bestPoint);
      placedLabels.push(bestRect);
    }

    return result;
  }

  let plannedPartLabels = $derived(planPartLabels(frame?.parts ?? []));

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
  class="grid place-items-center bg-base-200 p-3 text-base-content"
  style:width={hasPanPadding ? `calc(100% + ${displayWidth * renderedZoom + 24}px)` : `max(100%, ${displayWidth * renderedZoom + 24}px)`}
  style:height={hasPanPadding ? `calc(100% + ${displayHeight * renderedZoom + 24}px)` : `max(100%, ${displayHeight * renderedZoom + 24}px)`}
  data-theme="corporate"
  role="presentation"
  onclick={() => onSelect(null)}
>
  <svg
    width={displayWidth * renderedZoom}
    height={displayHeight * renderedZoom}
    viewBox="0 0 {boardWidth} {boardHeight}"
    style:overflow="visible"
    role="img"
    aria-label={ui.boardPreview.preview(preset === "hole170" ? ui.boardPreview.hole170 : preset === "hole400" ? ui.boardPreview.hole400 : ui.boardPreview.hole800)}
    class="block max-w-none"
  >
    <rect x="0.8" y="0.8" width={boardWidth - 1.6} height={boardHeight - 1.6} rx="7" fill="var(--color-base-100)" stroke="var(--color-base-300)" stroke-width="1.2" />

    {#if isMini}
      {#if !upperHalfOnly}
        <rect x={xInset - 5} y="78.05" width={boardWidth - 2 * xInset + 10} height="12.1" rx="2" fill="var(--color-base-300)" />
        <path d="M {xInset - 5} 78.6 H {boardWidth - xInset + 5}" stroke="var(--color-base-content)" stroke-opacity="0.3" stroke-width="1" />
      {/if}

      {#each columns as column}
        {#each mainRows as row}
          <g transform="translate({xInset + column * pitch} {18.1 + row * pitch})">
            <circle r="3.6" fill="var(--color-base-100)" stroke="var(--color-base-300)" stroke-width="1" />
            <circle r="1.6" fill="var(--color-base-content)" />
          </g>
          {#if !upperHalfOnly}
            <g transform="translate({xInset + column * pitch} {102.1 + row * pitch})">
              <circle r="3.6" fill="var(--color-base-100)" stroke="var(--color-base-300)" stroke-width="1" />
              <circle r="1.6" fill="var(--color-base-content)" />
            </g>
          {/if}
        {/each}
      {/each}
    {:else}
      <path d="M 1 4 H {boardWidth - 1}" stroke="var(--color-primary)" stroke-width="1.4" opacity="0.9" />
      <path d="M 1 32 H {boardWidth - 1}" stroke="var(--color-error)" stroke-width="1.4" opacity="0.9" />
      {#if !upperHalfOnly}
        <path d="M 1 220 H {boardWidth - 1}" stroke="var(--color-primary)" stroke-width="1.4" opacity="0.9" />
        <path d="M 1 248 H {boardWidth - 1}" stroke="var(--color-error)" stroke-width="1.4" opacity="0.9" />
        <path d="M 1 35 H {boardWidth - 1} M 1 217 H {boardWidth - 1}" stroke="var(--color-base-content)" stroke-opacity="0.3" stroke-width="1" />
      {:else}
        <path d="M 1 35 H {boardWidth - 1}" stroke="var(--color-base-content)" stroke-opacity="0.3" stroke-width="1" />
      {/if}

      {#if !upperHalfOnly}
        <rect x="1" y="118.7" width={boardWidth - 2} height="12" fill="var(--color-base-300)" />
        <path d="M 1 119.2 H {boardWidth - 1} M 1 130.2 H {boardWidth - 1}" stroke="var(--color-base-content)" stroke-opacity="0.3" stroke-width="1" />
      {/if}

      {#each columns as column}
        {#each mainRows as row}
          <g transform="translate({xInset + column * pitch} {60 + row * pitch})">
            <circle r="3.6" fill="var(--color-base-100)" stroke="var(--color-base-300)" stroke-width="1" />
            <circle r="1.6" fill="var(--color-base-content)" />
          </g>
          {#if !upperHalfOnly}
            <g transform="translate({xInset + column * pitch} {144 + row * pitch})">
              <circle r="3.6" fill="var(--color-base-100)" stroke="var(--color-base-300)" stroke-width="1" />
              <circle r="1.6" fill="var(--color-base-content)" />
            </g>
          {/if}
        {/each}
      {/each}

      {#each powerColumns as column}
        {#each upperHalfOnly ? [12, 24] : [12, 24, 228, 240] as y}
          <g transform="translate({xInset + railOffset + column * pitch} {y})">
            <circle r="3.6" fill="var(--color-base-100)" stroke="var(--color-base-300)" stroke-width="1" />
            <circle r="1.6" fill="var(--color-base-content)" />
          </g>
        {/each}
      {/each}

      <g font-family="ui-sans-serif, system-ui, sans-serif" font-size="7" font-weight="700" text-anchor="middle">
        <text x="7" y="14.5" fill="var(--color-primary)">−</text>
        <text x="7" y="26.5" fill="var(--color-error)">+</text>
        {#if !upperHalfOnly}
          <text x="7" y="230.5" fill="var(--color-primary)">−</text>
          <text x="7" y="242.5" fill="var(--color-error)">+</text>
        {/if}
      </g>
    {/if}

    {#if frame || plannedWires.length > 0}
      <g aria-label={ui.boardPreview.wires}>
        {#each plannedWires as planned (planned.wire.id)}
          {@const wire = planned.wire}
          {@const path = wirePath(planned)}
          {@const completed = completedWireIds.includes(wire.id)}
          <g
            class="cursor-pointer"
            role="button"
            tabindex="0"
            aria-label={ui.boardPreview.selectWire(wire.net_name ?? wire.net_id ?? wire.id)}
            onclick={(event) => selectWire(event, wire)}
            onkeydown={(event) => {
              if (event.key === "Enter" || event.key === " ") selectWire(event, wire);
            }}
          >
            <path
              d={path}
              fill="none"
              stroke={wire.kind === "air" ? "var(--color-neutral)" : wire.color ?? "var(--color-primary)"}
              stroke-width={(selected?.type === "wire" && selected.id === wire.id) || (selected?.type === "net" && selected.id === wire.net_id) ? 5 : completed ? 3 : wire.kind === "rail-tie" ? 2.8 : wire.kind === "routed" ? 2.2 : 1.2}
              stroke-dasharray={solidWires ? undefined : wire.kind === "air" || !completed ? "5 4" : undefined}
              stroke-linecap="round"
              opacity={selected ? ((selected.type === "wire" && selected.id === wire.id) || (selected.type === "net" && selected.id === wire.net_id) ? 1 : 0.14) : solidWires ? 0.95 : completed ? 0.95 : 0.38}
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

      {#if frame}
      <g aria-label={ui.boardPreview.components}>
        {#each frame.parts as part (part.id)}
          {@const bounds = partBounds(part)}
          {@const label = plannedPartLabels.get(part.id) ?? { x: bounds.cx, y: bounds.cy }}
          <g
            class="cursor-pointer transition-opacity"
            role="button"
            tabindex="0"
            aria-label={ui.boardPreview.selectComponent(part.reference)}
            opacity={selected?.type === "component" ? (selected.id === part.reference ? 1 : 0.25) : 1}
            onclick={(event) => selectComponent(event, part.reference)}
            onkeydown={(event) => {
              if (event.key === "Enter" || event.key === " ") selectComponent(event, part.reference);
            }}
          >
          {#if part.package === "axial" && part.pins.length >= 2}
            {@const axial = axialGeometry(part)}
            {#if axial}
              <line x1={axial.first.x} y1={axial.first.y} x2={axial.last.x} y2={axial.last.y} stroke="var(--color-neutral)" stroke-width="1.4" />
              <g transform="translate({axial.cx} {axial.cy}) rotate({axial.angle})">
                <rect
                  x={-axial.bodyWidth / 2}
                  y="-4.5"
                  width={axial.bodyWidth}
                  height="9"
                  rx={part.device === "diode" ? 2 : 3}
                  fill={part.device === "diode" ? "var(--color-neutral)" : "var(--color-warning)"}
                  stroke="var(--color-neutral)"
                  stroke-width="1"
                />
                {#if part.device === "diode"}
                  {@const bandX = cathodeBandX(part, axial.bodyWidth)}
                  {#if bandX !== null}
                    <rect
                      x={bandX - 1.2}
                      y="-4.1"
                      width="2.4"
                      height="8.2"
                      rx="0.6"
                      fill="var(--color-neutral-content)"
                    />
                  {/if}
                {/if}
              </g>
            {/if}
          {:else if part.package === "dip"}
            <path
              d={dipBodyPath(part)}
              fill="var(--color-neutral)"
              stroke="var(--color-base-content)"
              stroke-width="1.2"
              stroke-linejoin="round"
            />
          {:else}
            <rect
              x={bounds.x}
              y={bounds.y}
              width={bounds.width}
              height={bounds.height}
              rx="4"
              fill="var(--color-base-200)"
              stroke="var(--color-neutral)"
              stroke-width="1.2"
            />
          {/if}

          {#each part.pins as pin}
            {@const point = holePosition(pin.hole)}
            <g>
              {#if pin.number === "1" && part.pins.length > 2}
                <rect
                  x={point.x - 2.1}
                  y={point.y - 2.1}
                  width="4.2"
                  height="4.2"
                  rx="0.55"
                  fill={selected?.type === "net" && selected.id === pin.net_id ? "var(--color-warning)" : "var(--color-base-100)"}
                  stroke="var(--color-warning-content)"
                  stroke-width="0.8"
                />
              {:else}
                <circle
                  cx={point.x}
                  cy={point.y}
                  r={selected?.type === "net" && selected.id === pin.net_id ? 4 : 2.4}
                  fill={selected?.type === "net" && selected.id === pin.net_id ? "var(--color-warning)" : "var(--color-base-100)"}
                  stroke="var(--color-neutral)"
                  stroke-width="0.8"
                />
              {/if}
              <title>{part.reference} pin {pinLabelText(pin)}</title>
            </g>
          {/each}

          {#if part.package === "dip"}
            {@const dot = pinOneDot(part)}
            {#if dot}
              <circle cx={dot.x} cy={dot.y} r="1.7" fill="var(--color-warning)" stroke="var(--color-neutral-content)" stroke-width="0.7" />
            {/if}
          {/if}

          {#if part.device === "diode" || part.device === "led"}
            {#each part.pins.filter((pin) => ["A", "K"].includes(normalizedPinName(pin) ?? "")) as pin}
              {@const point = polarityLabelPoint(part, pin)}
              <text
                x={point.x}
                y={point.y}
                text-anchor="middle"
                dominant-baseline="central"
                font-family="ui-sans-serif, system-ui, sans-serif"
                font-size="6.5"
                font-weight="800"
                fill="var(--color-base-content)"
                stroke="var(--color-base-100)"
                stroke-width="2.4"
                paint-order="stroke"
                pointer-events="none"
              >{normalizedPinName(pin)}</text>
            {/each}
          {/if}
          <text
            x={label.x}
            y={label.y}
            text-anchor="middle"
            dominant-baseline="central"
            font-family="ui-sans-serif, system-ui, sans-serif"
            font-size="6.5"
            font-weight="700"
            fill={hasDarkBody(part) ? "var(--color-neutral-content)" : "var(--color-base-content)"}
            stroke={hasDarkBody(part) ? "var(--color-neutral)" : "var(--color-base-100)"}
            stroke-width="2.4"
            stroke-linejoin="round"
            paint-order="stroke"
            pointer-events="none"
          >{part.reference}</text>

          <title>{part.reference}{part.value ? ` · ${part.value}` : ""}</title>
          </g>
        {/each}
      </g>

      {#if selected?.type === "component"}
        {@const selectedPart = frame.parts.find((part) => part.reference === selected.id)}
        {#if selectedPart}
          {@const pinLabels = planSelectedPinLabels(selectedPart)}
          <g aria-label={ui.boardPreview.pinDefinitions(selectedPart.reference)} pointer-events="none">
            <!-- 所有引线统一置于标签下层，并只画到标签边框，避免穿过任何标签。 -->
            <g aria-hidden="true">
              {#each pinLabels as pinLabel}
                {@const leaderEnd = calloutLeaderEnd(pinLabel)}
                <line
                  x1={pinLabel.point.x}
                  y1={pinLabel.point.y}
                  x2={leaderEnd.x}
                  y2={leaderEnd.y}
                  stroke="var(--color-warning-content)"
                  stroke-width="0.9"
                  stroke-dasharray="2 1.5"
                  opacity="0.85"
                />
              {/each}
            </g>

            <g>
              {#each pinLabels as pinLabel}
                <rect
                  x={pinLabel.x - pinLabel.width / 2}
                  y={pinLabel.y - pinLabel.height / 2}
                  width={pinLabel.width}
                  height={pinLabel.height}
                  rx="2.5"
                  fill="var(--color-base-100)"
                  fill-opacity="1"
                  stroke="var(--color-warning-content)"
                  stroke-width="0.9"
                />
                <text
                  x={pinLabel.x}
                  y={pinLabel.y}
                  text-anchor="middle"
                  dominant-baseline="central"
                  font-family="ui-sans-serif, system-ui, sans-serif"
                  font-size="5.8"
                  font-weight="700"
                  fill="var(--color-base-content)"
                >{pinLabel.text}</text>
              {/each}
            </g>
          </g>
        {/if}
      {/if}
      {/if}
    {/if}
  </svg>
</div>
