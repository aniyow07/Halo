import { describe, expect, it } from "vitest";
import { computePanelPosition, isUsableSelection, rectFromPoints } from "./panelPosition";

const vp = { width: 1600, height: 1000 };
const panel = { width: 420, height: 500 };

describe("computePanelPosition", () => {
  it("prefers the right of the selection", () => {
    expect(computePanelPosition({ x: 100, y: 100, width: 200, height: 100 }, panel, vp)).toEqual({ left: 312, top: 100 });
  });
  it("falls back to the left near the right edge", () => {
    const p = computePanelPosition({ x: 1200, y: 100, width: 300, height: 100 }, panel, vp);
    expect(p).toEqual({ left: 1200 - 12 - 420, top: 100 });
  });
  it("goes below a full-width selection", () => {
    const p = computePanelPosition({ x: 0, y: 50, width: 1600, height: 200 }, panel, vp);
    expect(p).toEqual({ left: 12, top: 262 });
  });
  it("goes above a full-width selection at the bottom", () => {
    const p = computePanelPosition({ x: 0, y: 700, width: 1600, height: 250 }, panel, vp);
    expect(p).toEqual({ left: 12, top: 700 - 12 - 500 });
  });
  it("stays inside the screen when the selection covers everything", () => {
    const p = computePanelPosition({ x: 0, y: 0, width: 1600, height: 1000 }, panel, vp);
    expect(p.left + panel.width).toBeLessThanOrEqual(vp.width);
    expect(p.top + panel.height).toBeLessThanOrEqual(vp.height);
    expect(p.left).toBeGreaterThanOrEqual(0);
    expect(p.top).toBeGreaterThanOrEqual(0);
  });
  it("clamps the vertical alignment to the screen", () => {
    const p = computePanelPosition({ x: 100, y: 900, width: 100, height: 50 }, panel, vp);
    expect(p.top).toBe(1000 - 500 - 12);
  });
});

describe("selection helpers", () => {
  it("normalises drags in any direction", () => {
    expect(rectFromPoints(50, 60, 10, 20)).toEqual({ x: 10, y: 20, width: 40, height: 40 });
  });
  it("rejects tiny selections", () => {
    expect(isUsableSelection({ x: 0, y: 0, width: 3, height: 100 })).toBe(false);
    expect(isUsableSelection({ x: 0, y: 0, width: 8, height: 8 })).toBe(true);
  });
});
