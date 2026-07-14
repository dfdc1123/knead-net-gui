export function centerCanvasNow(viewport: HTMLDivElement) {
  viewport.scrollLeft = (viewport.scrollWidth - viewport.clientWidth) / 2;
  viewport.scrollTop = (viewport.scrollHeight - viewport.clientHeight) / 2;
}

export function centerCanvas(viewport: HTMLDivElement) {
  let centered = false;
  let animationFrame = 0;

  const centerWhenReady = () => {
    cancelAnimationFrame(animationFrame);
    animationFrame = requestAnimationFrame(() => {
      const diagram = viewport.querySelector("svg");
      const diagramBounds = diagram?.getBoundingClientRect();
      if (
        centered ||
        viewport.clientWidth === 0 ||
        viewport.clientHeight === 0 ||
        !diagramBounds?.width ||
        !diagramBounds.height
      ) return;

      centerCanvasNow(viewport);
      centered = true;
      resizeObserver.disconnect();
      mutationObserver.disconnect();
    });
  };

  // 各步骤会先在 display:none 状态下挂载，SVG 也可能稍后才出现。
  // 同时监听视口尺寸和子节点，等画布真正可见后只执行一次默认居中。
  const resizeObserver = new ResizeObserver(centerWhenReady);
  const mutationObserver = new MutationObserver(centerWhenReady);
  resizeObserver.observe(viewport);
  mutationObserver.observe(viewport, { childList: true, subtree: true });
  centerWhenReady();

  return {
    destroy() {
      cancelAnimationFrame(animationFrame);
      resizeObserver.disconnect();
      mutationObserver.disconnect();
    },
  };
}
