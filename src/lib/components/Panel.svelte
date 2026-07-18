<script lang="ts">
  import type { Snippet } from "svelte";
  import type { HTMLAttributes } from "svelte/elements";

  type PanelProps = HTMLAttributes<HTMLElement> & {
    as?: "aside" | "section";
    children: Snippet;
  };

  let {
    as = "section",
    children,
    class: className = "",
    ...attributes
  }: PanelProps = $props();
</script>

<svelte:element
  this={as}
  class="card min-h-0 border border-base-300 bg-base-100 shadow-sm {className}"
  {...attributes}
>
  {@render children()}
</svelte:element>
