<script lang="ts">
  import "../app.css";
  import {
    hasShortcutModifier,
    isAuraActivationKey,
    isTextEditingTarget,
  } from "$lib/keyboardShortcuts.js";
  let { children } = $props();

  function preventContextMenu(event: MouseEvent) {
    event.preventDefault();
  }

  function auraActionButton() {
    for (const aura of document.querySelectorAll<HTMLElement>(".workflow-next-step")) {
      if (aura.getClientRects().length === 0) continue;
      const button = aura.matches("button")
        ? (aura as HTMLButtonElement)
        : aura.closest<HTMLButtonElement>("button") ??
          aura.querySelector<HTMLButtonElement>("button");
      if (button && !button.disabled && button.getClientRects().length > 0) return button;
    }
    return null;
  }

  function triggerAuraAction(event: KeyboardEvent) {
    if (
      event.defaultPrevented ||
      event.repeat ||
      hasShortcutModifier(event) ||
      !isAuraActivationKey(event.key) ||
      isTextEditingTarget(event.target)
    ) return;

    const target = event.target instanceof Element ? event.target : null;
    if (target?.closest("button, a, [role='button']")) return;

    const button = auraActionButton();
    if (!button) return;
    const openDialog = document.querySelector<HTMLDialogElement>("dialog[open]");
    if (openDialog && !openDialog.contains(button)) return;

    event.preventDefault();
    button.click();
  }
</script>

<svelte:window oncontextmenu={preventContextMenu} onkeydown={triggerAuraAction} />

{@render children()}
