<script lang="ts">
  // QOL-10: Ctrl+V outside a text box goes to the position of a FEN on the
  // clipboard. A page only gets the clipboard in a paste event, and WebKit
  // fires none where nothing is editable, so Ctrl+V first moves the focus
  // to this hidden text box: the paste lands here and the focus goes back.
  import { goToPosition } from "../gotoPosition";
  import { notices } from "../stores/notify.svelte";

  let sink: HTMLTextAreaElement;
  let returnTo: HTMLElement | null = null;
  let timer: ReturnType<typeof setTimeout> | undefined;

  function editable(target: EventTarget | null) {
    const el = target as HTMLElement | null;
    return (
      !!el && el !== sink && (el.isContentEditable || ["INPUT", "TEXTAREA", "SELECT"].includes(el.tagName))
    );
  }

  const modalOpen = () => !!document.querySelector("[aria-modal='true']");

  function onKeydown(e: KeyboardEvent) {
    if (!(e.ctrlKey || e.metaKey) || e.altKey || e.key.toLowerCase() !== "v") return;
    if (editable(e.target) || modalOpen()) return;
    const active = document.activeElement;
    returnTo = active instanceof HTMLElement && active !== document.body ? active : null;
    sink.focus({ preventScroll: true });
    // The paste follows at once; if none comes, don't keep the keys.
    clearTimeout(timer);
    timer = setTimeout(giveBack, 500);
  }

  function giveBack() {
    clearTimeout(timer);
    if (document.activeElement === sink) {
      if (returnTo?.isConnected) returnTo.focus({ preventScroll: true, focusVisible: false });
      else sink.blur();
    }
    returnTo = null;
  }

  function onPaste(e: ClipboardEvent) {
    if (e.target !== sink && (editable(e.target) || modalOpen())) return;
    e.preventDefault();
    giveBack();
    const text = e.clipboardData?.getData("text/plain") ?? "";
    if (text.trim()) void paste(text);
  }

  async function paste(text: string) {
    const problem = await goToPosition(text);
    if (problem?.kind === "noFen") {
      notices.show("There's no FEN on the clipboard. Copy a position's FEN, then press Ctrl+V to go to it.");
    } else if (problem?.kind === "invalid") {
      notices.show(`Can't go to the pasted position: ${problem.error}.`, "error");
    }
  }
</script>

<svelte:window onkeydown={onKeydown} onpaste={onPaste} />

<textarea bind:this={sink} class="sink" tabindex="-1" aria-hidden="true"></textarea>

<style>
  /* Focusable (so it takes the paste) but never seen or clicked. */
  .sink {
    position: fixed;
    top: 0;
    left: 0;
    width: 1px;
    height: 1px;
    padding: 0;
    border: 0;
    opacity: 0;
    resize: none;
    pointer-events: none;
  }
</style>
