<script lang="ts">
  import Dock from "$lib/components/Dock.svelte";
  import Step1SelectFiles from "$lib/components/Step1SelectFiles.svelte";
  import Step2SelectBoard from "$lib/components/Step2SelectBoard.svelte";
  import Step3Compute from "$lib/components/Step3Compute.svelte";
  import Step4Result from "$lib/components/Step4Result.svelte";
  import type { BreadboardSelection } from "$lib/layout";

  let step = $state(0);
  let sourceReady = $state(false);
  let boardReady = $state(false);
  let resultReady = $state(false);
  let board = $state<BreadboardSelection | null>(null);

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
  }

  function handleBoardStatus(ready: boolean) {
    boardReady = ready;
    resultReady = false;
  }

  function handleBoardChange(selection: BreadboardSelection | null) {
    board = selection;
  }

  function handleComputeComplete() {
    resultReady = true;
  }
</script>

<div class="h-screen flex flex-col bg-base-100">
  <main class="flex-1 overflow-auto">
    <div class:hidden={step !== 0} class="h-full">
      <Step1SelectFiles onStatusChange={handleSourceStatus} />
    </div>
    {#if sourceReady}
      <div class:hidden={step !== 1} class="h-full">
        <Step2SelectBoard onStatusChange={handleBoardStatus} onBoardChange={handleBoardChange} />
      </div>
    {/if}
    {#if sourceReady && boardReady && board}
      <div class:hidden={step !== 2} class="h-full">
        <Step3Compute preset={board.preset} cols={board.cols} onComplete={handleComputeComplete} />
      </div>
    {/if}
    {#if resultReady}
      <div class:hidden={step !== 3} class="h-full">
        <Step4Result />
      </div>
    {/if}
  </main>

  <Dock bind:current={step} enabled={enabledSteps} />
</div>
