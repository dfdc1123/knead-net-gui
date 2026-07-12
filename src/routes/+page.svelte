<script lang="ts">
  import Dock from "$lib/components/Dock.svelte";
  import Step1SelectFiles from "$lib/components/Step1SelectFiles.svelte";
  import Step2SelectBoard from "$lib/components/Step2SelectBoard.svelte";
  import Step3Compute from "$lib/components/Step3Compute.svelte";
  import Step4Result from "$lib/components/Step4Result.svelte";

  let step = $state(0);
</script>

<div class="h-screen flex flex-col bg-base-100">
  <main class="flex-1 overflow-auto">
    {#if step === 0}
      <Step1SelectFiles />
    {:else if step === 1}
      <Step2SelectBoard />
    {:else if step === 2}
      <Step3Compute />
    {:else}
      <Step4Result />
    {/if}
  </main>

  <Dock bind:current={step} />
</div>