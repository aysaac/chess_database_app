<script lang="ts">
  // A button of the ribbon (UI-18). Large: the icon above a one- or
  // two-line label. Small: icon and label on one line; `compact` hides the
  // label (it stays the accessible name and the tooltip). Group: a
  // collapsed group's button, its name under the icon.
  import type { Snippet } from "svelte";
  import type { HTMLButtonAttributes } from "svelte/elements";
  import type { IconName } from "../icons/lucide";
  import Icon from "./Icon.svelte";

  let {
    icon,
    label,
    size = "small",
    compact = false,
    primary = false,
    title,
    element = $bindable(),
    after,
    ...rest
  }: HTMLButtonAttributes & {
    icon: IconName;
    label: string;
    size?: "small" | "large" | "group";
    compact?: boolean;
    /** Highlighted, as Save is while the game has unsaved changes. */
    primary?: boolean;
    element?: HTMLButtonElement;
    /** Shown after the label, such as a dropdown's chevron. */
    after?: Snippet;
  } = $props();
</script>

<button
  bind:this={element}
  class="rb {size}"
  class:compact={compact && size === "small"}
  class:primary
  aria-label={label}
  title={title ?? label}
  {...rest}
>
  <Icon name={icon} size={size === "small" ? 16 : 24} />
  <span class="label">{label}</span>
  {@render after?.()}
</button>

<style>
  .rb {
    position: relative;
    display: flex;
    align-items: center;
    gap: 5px;
    padding: 0 6px;
    font-size: 13px;
    line-height: 1.15;
  }

  .small {
    height: 24px;
  }

  .small.compact {
    padding: 0 5px;
  }

  /* Hidden but still read out (and found by the smoke drivers). */
  .small.compact .label {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip-path: inset(50%);
    white-space: nowrap;
  }

  .large,
  .group {
    flex-direction: column;
    justify-content: flex-start;
    gap: 4px;
    height: 100%;
    padding: 6px 5px 4px;
  }

  .large .label,
  .group .label {
    max-width: 60px;
    text-align: center;
    white-space: normal;
  }

  .group {
    min-width: 58px;
  }

  .rb[aria-pressed="true"] {
    background: var(--felt-soft);
    box-shadow: inset 0 0 0 1px var(--felt);
  }
</style>
