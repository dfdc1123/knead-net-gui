<script lang="ts">
  import Dock from "$lib/components/Dock.svelte";
  import Step1SelectFiles from "$lib/components/Step1SelectFiles.svelte";
  import Step2SelectBoard from "$lib/components/Step2SelectBoard.svelte";
  import Step3Compute from "$lib/components/Step3Compute.svelte";
  import Step4Result from "$lib/components/Step4Result.svelte";
  import type { BreadboardSelection, LayoutFrame } from "$lib/layout";

  let step = $state(0);
  let sourceReady = $state(false);
  let boardReady = $state(false);
  let resultReady = $state(false);
  let board = $state<BreadboardSelection | null>(null);
  let schematicSvg = $state("");
  let resultFrame = $state<LayoutFrame | null>(null);

  const enabledSteps = $derived([
    true,
    sourceReady,
    sourceReady && boardReady,
    resultReady,
  ]);

  function handleSourceStatus(ready: boolean) {
    sourceReady = ready;
    boardReady = false;
    board = null;
    resultReady = false;
    resultFrame = null;
  }

  function handleBoardStatus(ready: boolean) {
    boardReady = ready;
    resultReady = false;
    resultFrame = null;
  }

  function handleBoardChange(selection: BreadboardSelection | null) {
    board = selection;
  }

  function handleComputeComplete(frame: LayoutFrame) {
    resultFrame = frame;
    resultReady = true;
  }
</script>

<div class="flex h-screen flex-col bg-base-200">
  <main class="min-h-0 flex-1 pb-16">
    <div class:hidden={step !== 0} class="h-full">
      <Step1SelectFiles onStatusChange={handleSourceStatus} onSchematicChange={(svg) => (schematicSvg = svg)} />
    </div>
    {#if sourceReady}
      <div class:hidden={step !== 1} class="h-full">
        <Step2SelectBoard onStatusChange={handleBoardStatus} onBoardChange={handleBoardChange} />
      </div>
    {/if}
    {#if sourceReady && boardReady && board}
      <div class:hidden={step !== 2} class="h-full">
        <Step3Compute preset={board.preset} boardCols={board.boardCols} upperHalfOnly={board.upperHalfOnly} onComplete={handleComputeComplete} />
      </div>
    {/if}
    {#if resultReady && board && resultFrame}
      <div class:hidden={step !== 3} class="h-full">
        <Step4Result
          preset={board.preset}
          upperHalfOnly={board.upperHalfOnly}
          frame={resultFrame}
          {schematicSvg}
        />
      </div>
    {/if}
  </main>

  <Dock bind:current={step} enabled={enabledSteps} />
</div>
