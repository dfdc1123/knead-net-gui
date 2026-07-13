<script lang="ts">
  type Preset = "hole170" | "hole400" | "hole800";

  let { preset, cols }: { preset: Preset; cols: number } = $props();

  const pitch = 12;
  const mainRows = [0, 1, 2, 3, 4];

  function range(length: number) {
    return Array.from({ length }, (_, index) => index);
  }

  function railColumns(kind: Preset, columnCount: number) {
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

  // 400 孔参考板的首个电源轨孔位于主区第 1、2 列之间之后：
  // (31 - 13.65) / 9 ≈ 1.928 个孔距。这只是绘图坐标；算法仍然
  // 使用它自己的整数列坐标。
  let railOffset = $derived(preset === "hole400" ? pitch * 1.928 : 0);
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
</script>

<div class="flex min-w-full justify-center p-3">
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
  </svg>
</div>
