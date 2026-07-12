<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";

  let name = $state("");
  let greetMsg = $state("");

  async function greet(event: Event) {
    event.preventDefault();
    greetMsg = await invoke("greet", { name });
  }
</script>

<main class="min-h-screen flex flex-col items-center justify-center gap-6 bg-base-200 p-8">
  <h1 class="text-4xl font-bold">Welcome to Tauri + Svelte</h1>

  <div class="flex gap-4">
    <a href="https://vite.dev" target="_blank" class="link link-hover">Vite</a>
    <a href="https://tauri.app" target="_blank" class="link link-hover">Tauri</a>
    <a href="https://svelte.dev" target="_blank" class="link link-hover">SvelteKit</a>
  </div>

  <form class="flex gap-2" onsubmit={greet}>
    <input
      id="greet-input"
      class="input input-bordered w-64"
      placeholder="Enter a name..."
      bind:value={name}
    />
    <button type="submit" class="btn btn-primary">Greet</button>
  </form>

  {#if greetMsg}
    <p class="text-lg">{greetMsg}</p>
  {/if}
</main>