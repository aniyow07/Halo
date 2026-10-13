import type { Rect, Size } from "../types";

export const PANEL_MARGIN = 12;

/**
 * Place the floating panel next to the selection without covering it when
 * possible, always fully inside the viewport. Preference order: right,
 * left, below, above; otherwise overlap, clamped to the screen.
 */
export function computePanelPosition(
  selection: Rect,
  panel: Size,
  viewport: Size,
  margin = PANEL_MARGIN,
): { left: number; top: number } {
  const maxLeft = Math.max(margin, viewport.width - panel.width - margin);
  const maxTop = Math.max(margin, viewport.height - panel.height - margin);
  const clamp = (v: number, lo: number, hi: number) => Math.min(Math.max(v, lo), hi);
  const alignedTop = clamp(selection.y, margin, maxTop);
  const alignedLeft = clamp(selection.x, margin, maxLeft);

  const right = selection.x + selection.width + margin;
  if (right + panel.width + margin <= viewport.width) return { left: right, top: alignedTop };

  const left = selection.x - margin - panel.width;
  if (left >= margin) return { left, top: alignedTop };

  const below = selection.y + selection.height + margin;
  if (below + panel.height + margin <= viewport.height) return { left: alignedLeft, top: below };

  const above = selection.y - margin - panel.height;
  if (above >= margin) return { left: alignedLeft, top: above };

  return { left: maxLeft, top: clamp(selection.y, margin, maxTop) };
}

/** Normalise a drag (which may go in any direction) into a rectangle. */
export function rectFromPoints(ax: number, ay: number, bx: number, by: number): Rect {
  return {
    x: Math.min(ax, bx),
    y: Math.min(ay, by),
    width: Math.abs(bx - ax),
    height: Math.abs(by - ay),
  };
}

export const MIN_SELECTION_CSS = 8;

export function isUsableSelection(rect: Rect): boolean {
  return rect.width >= MIN_SELECTION_CSS && rect.height >= MIN_SELECTION_CSS;
}
