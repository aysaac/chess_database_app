/**
 * Fixed-position coordinates for a popover opening under `anchor` (a move in
 * the notation), or above it when there's no room below. Keeps it on screen.
 */
export function placeUnder(anchor: HTMLElement, box: HTMLElement): { left: number; top: number } {
  const a = anchor.getBoundingClientRect();
  const { width, height } = box.getBoundingClientRect();
  const below = a.bottom + 6;
  return {
    left: Math.max(8, Math.min(a.left, window.innerWidth - width - 8)),
    top: below + height <= window.innerHeight - 8 ? below : Math.max(8, a.top - height - 6),
  };
}
