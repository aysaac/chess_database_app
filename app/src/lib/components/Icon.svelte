<script lang="ts">
  // A vendored Lucide icon (lib/icons), drawn in the text colour.
  import { ICONS, type IconName } from "../icons/lucide";

  let { name, size = 16 }: { name: IconName; size?: number } = $props();
</script>

<svg
  class="icon"
  width={size}
  height={size}
  viewBox="0 0 24 24"
  fill="none"
  stroke="currentColor"
  stroke-width="2"
  stroke-linecap="round"
  stroke-linejoin="round"
  aria-hidden="true"
  focusable="false">{@html ICONS[name]}</svg
>

<style>
  .icon {
    flex: none;
    display: block;
  }
</style>
