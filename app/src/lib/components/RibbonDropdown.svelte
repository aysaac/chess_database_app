<script lang="ts">
  // A ribbon button that opens a dropdown (UI-18): a menu (Board image,
  // Recent, New game, the reference database), or for a collapsed group a
  // panel with the group's buttons. Menus follow the WAI-ARIA menu button
  // pattern: opening focuses the first item, ↑/↓ (and Home/End) move, Enter
  // or Space picks, Escape closes and puts the focus back on the button.
  // A click on an item closes the dropdown, except on items that open
  // another menu and items marked `data-keep-open`.
  import { tick, type Snippet } from "svelte";
  import type { IconName } from "../icons/lucide";
  import Icon from "./Icon.svelte";
  import RibbonButton from "./RibbonButton.svelte";

  let {
    icon,
    label,
    title,
    size = "small",
    compact = false,
    disabled = false,
    kind = "menu",
    menuLabel,
    onopen,
    onmain,
    mainLabel,
    mainTitle,
    class: className = "",
    children,
  }: {
    icon: IconName;
    /** The trigger's text: the database name for the reference dropdown. */
    label: string;
    title?: string;
    /** `wide` is the reference database's select-like box; `group` a collapsed group. */
    size?: "small" | "large" | "wide" | "group";
    compact?: boolean;
    disabled?: boolean;
    /** `menu`: menu items; `panel`: a collapsed group's buttons. */
    kind?: "menu" | "panel";
    menuLabel?: string;
    /** Runs as the dropdown opens (to load what it lists). */
    onopen?: () => unknown;
    /** A split button: the main part does this, the arrow under it opens the menu. */
    onmain?: () => unknown;
    mainLabel?: string;
    mainTitle?: string;
    class?: string;
    children: Snippet;
  } = $props();

  const id = $props.id();
  let open = $state(false);
  let alignRight = $state(false);
  let root: HTMLDivElement;
  let trigger = $state<HTMLButtonElement>();
  let popup = $state<HTMLDivElement>();

  /** What ↑/↓ move between: this dropdown's items, not those of a menu open inside it. */
  function items(): HTMLElement[] {
    if (!popup) return [];
    const selector = kind === "menu" ? "[role^='menuitem']" : "button";
    return [...popup.querySelectorAll<HTMLButtonElement>(selector)].filter(
      (el) => !el.disabled && el.closest("[data-popup]") === popup,
    );
  }

  async function show(focus: "first" | "last" = "first") {
    if (open) return;
    open = true;
    alignRight = false;
    onopen?.();
    await tick();
    if (!popup) return;
    // Keep it in the window.
    alignRight = popup.getBoundingClientRect().right > window.innerWidth - 8;
    const list = items();
    (focus === "first" ? list[0] : list.at(-1))?.focus();
  }

  function close(refocus = true) {
    if (!open) return;
    open = false;
    if (refocus) trigger?.focus();
  }

  function onKey(e: KeyboardEvent) {
    if (!open) {
      if (e.target === trigger && (e.key === "ArrowDown" || e.key === "ArrowUp")) {
        e.preventDefault();
        e.stopPropagation();
        void show(e.key === "ArrowDown" ? "first" : "last");
      }
      return;
    }
    const list = items();
    const i = list.indexOf(document.activeElement as HTMLElement);
    let next: HTMLElement | undefined;
    switch (e.key) {
      case "Escape":
        e.preventDefault();
        e.stopPropagation();
        close();
        return;
      case "ArrowDown":
        next = list[(i + 1) % list.length];
        break;
      case "ArrowUp":
        next = list[i <= 0 ? list.length - 1 : i - 1];
        break;
      case "Home":
        next = list[0];
        break;
      case "End":
        next = list.at(-1);
        break;
      case "ArrowLeft":
      case "ArrowRight":
        // Not a move through the game while a menu has the focus.
        e.stopPropagation();
        return;
      case "Tab":
        if (kind === "menu") close(false);
        return;
      default:
        return;
    }
    e.preventDefault();
    e.stopPropagation();
    next?.focus();
  }

  /** Closes after an item did its action (its own handler runs first). */
  function onPopupClick(e: MouseEvent) {
    const button = (e.target as HTMLElement).closest("button");
    if (
      !button ||
      button.disabled ||
      button.closest("[data-popup]") !== popup ||
      button.hasAttribute("aria-expanded") ||
      button.hasAttribute("data-keep-open")
    ) {
      return;
    }
    close();
  }

  function onWindowDown(e: MouseEvent) {
    if (open && !root.contains(e.target as Node)) close(false);
  }

  // An Escape that reached the window unhandled: the focus was nowhere in
  // particular (a click doesn't move it on every platform). Inside the
  // dropdown, `onKey` takes it first and stops it.
  function onWindowKey(e: KeyboardEvent) {
    if (open && e.key === "Escape") close(root.contains(document.activeElement));
  }

  function onFocusOut(e: FocusEvent) {
    // Only when the focus moves somewhere else: a click on a spot that
    // takes no focus mustn't close it before the click lands.
    const to = e.relatedTarget as Node | null;
    if (open && to && !root.contains(to)) close(false);
  }

  const triggerProps = $derived({
    "aria-haspopup": kind === "menu" ? ("menu" as const) : undefined,
    "aria-expanded": open,
    "aria-controls": open ? `${id}-popup` : undefined,
    disabled,
    onclick: () => (open ? close() : show()),
  });
</script>

<svelte:window onmousedown={onWindowDown} onkeydown={onWindowKey} />

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="dropdown {size} {className}" bind:this={root} onkeydown={onKey} onfocusout={onFocusOut}>
  {#if onmain}
    <div class="split">
      <RibbonButton
        size="large"
        {icon}
        label={mainLabel ?? label}
        title={mainTitle}
        {disabled}
        onclick={() => onmain()}
      />
      <button
        bind:this={trigger}
        class="arrow"
        aria-label={label}
        title={title ?? label}
        {...triggerProps}
      >
        <Icon name="chevron-down" size={14} />
      </button>
    </div>
  {:else if size === "wide"}
    <button bind:this={trigger} class="box" class:compact title={title ?? label} {...triggerProps}>
      <Icon name={icon} size={16} />
      <span class="name">{label}</span>
      <Icon name="chevron-down" size={14} />
    </button>
  {:else}
    <RibbonButton bind:element={trigger} {icon} {label} {title} {size} {compact} {...triggerProps}>
      {#snippet after()}<span class="chevron"><Icon name="chevron-down" size={size === "small" ? 12 : 14} /></span>{/snippet}
    </RibbonButton>
  {/if}

  {#if open}
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <div
      bind:this={popup}
      id="{id}-popup"
      class="popup {kind}"
      class:right={alignRight}
      data-popup
      role={kind === "menu" ? "menu" : "group"}
      aria-label={menuLabel ?? label}
      onclick={onPopupClick}
    >
      {@render children()}
    </div>
  {/if}
</div>

<style>
  .dropdown {
    position: relative;
    display: flex;
  }

  .dropdown.large,
  .dropdown.group {
    height: 100%;
  }

  .chevron {
    display: flex;
    opacity: 0.75;
  }

  .group .chevron {
    margin-top: -3px;
  }

  .split {
    display: flex;
    flex-direction: column;
    height: 100%;
  }

  .split :global(.rb.large) {
    flex: 1;
    min-height: 0;
    border-bottom-left-radius: 0;
    border-bottom-right-radius: 0;
  }

  .arrow {
    display: flex;
    justify-content: center;
    height: 16px;
    padding: 0;
    border-top-left-radius: 0;
    border-top-right-radius: 0;
  }

  /* The reference database: a select-like box. */
  .box {
    display: flex;
    align-items: center;
    gap: 6px;
    width: 196px;
    height: 24px;
    padding: 0 6px;
    background: var(--paper);
    border-color: var(--rule);
    font-size: 13px;
  }

  .box.compact {
    width: 150px;
  }

  .name {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    text-align: left;
  }

  .popup {
    position: absolute;
    top: calc(100% + 4px);
    left: 0;
    z-index: 60;
    padding: 5px;
    background: var(--paper);
    border: 1px solid var(--rule);
    border-radius: 6px;
    box-shadow: 0 6px 20px color-mix(in srgb, var(--ink) 18%, transparent);
  }

  .popup.right {
    left: auto;
    right: 0;
  }

  .popup.menu {
    display: grid;
    min-width: 210px;
    max-height: 70vh;
    overflow-y: auto;
  }

  .popup.menu :global(:is([role^="menuitem"])) {
    text-align: left;
  }

  .popup.panel {
    display: flex;
    padding: 4px;
  }
</style>
