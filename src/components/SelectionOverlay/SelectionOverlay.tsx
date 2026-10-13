import { useEffect, useRef, useState, type PointerEvent } from "react";
import { isUsableSelection, rectFromPoints } from "../../app/panelPosition";
import type { Rect, Size } from "../../types";
import "./SelectionOverlay.css";

export interface FrozenFrame {
  width: number;
  height: number;
  rgba: ArrayBuffer;
}

interface Props {
  frame: FrozenFrame | null;
  /** The confirmed selection, kept highlighted while the panel is open. */
  selection: Rect | null;
  /** `viewport` is the measured size of the surface the frame is drawn on,
   *  which is what the rectangle is relative to. */
  onSelect: (rect: Rect, viewport: Size) => void;
  onFramePainted?: () => void;
  hint?: string;
}

/** Full-screen selection surface drawn over the frozen screenshot. */
export function SelectionOverlay({ frame, selection, onSelect, onFramePainted, hint }: Props) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const [drag, setDrag] = useState<{ x: number; y: number; cx: number; cy: number } | null>(null);

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!frame || !canvas) return;
    canvas.width = frame.width;
    canvas.height = frame.height;
    const ctx = canvas.getContext("2d");
    if (ctx) {
      ctx.putImageData(new ImageData(new Uint8ClampedArray(frame.rgba), frame.width, frame.height), 0, 0);
    }
    // Not requestAnimationFrame: the window is still hidden at this point
    // and WebKit does not run animation frames for hidden pages.
    const timer = setTimeout(() => onFramePainted?.(), 0);
    return () => clearTimeout(timer);
  }, [frame, onFramePainted]);

  const onPointerDown = (e: PointerEvent<HTMLDivElement>) => {
    if (e.button !== 0) return;
    e.currentTarget.setPointerCapture?.(e.pointerId);
    setDrag({ x: e.clientX, y: e.clientY, cx: e.clientX, cy: e.clientY });
  };
  const onPointerMove = (e: PointerEvent<HTMLDivElement>) => {
    if (drag) setDrag({ ...drag, cx: e.clientX, cy: e.clientY });
  };
  const onPointerUp = (e: PointerEvent<HTMLDivElement>) => {
    if (!drag) return;
    // Measure the surface instead of trusting window.innerWidth, which
    // WebKitGTK can report stale after a fullscreen resize.
    const bounds = e.currentTarget.getBoundingClientRect();
    const rect = rectFromPoints(drag.x - bounds.left, drag.y - bounds.top, e.clientX - bounds.left, e.clientY - bounds.top);
    setDrag(null);
    const viewport = { width: bounds.width || window.innerWidth, height: bounds.height || window.innerHeight };
    if (isUsableSelection(rect)) onSelect(rect, viewport);
  };

  // Escape aborts an in-progress drag without dismissing Halo.
  useEffect(() => {
    if (!drag) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopImmediatePropagation();
        setDrag(null);
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [drag]);

  const live = drag ? rectFromPoints(drag.x, drag.y, drag.cx, drag.cy) : null;
  const shown = live ?? selection;

  return (
    <div
      className="selection-surface"
      data-testid="selection-surface"
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
      onPointerCancel={() => setDrag(null)}
    >
      <canvas ref={canvasRef} className="frozen-frame" aria-label="Frozen screen" />
      {shown ? (
        <>
          {/* Four dim panels around the selection (a huge box-shadow is not
              reliably painted by WebKitGTK). */}
          <div className="selection-dim" style={{ bottom: `calc(100% - ${shown.y}px)` }} />
          <div className="selection-dim" style={{ top: shown.y + shown.height }} />
          <div
            className="selection-dim"
            style={{ top: shown.y, height: shown.height, right: `calc(100% - ${shown.x}px)` }}
          />
          <div
            className="selection-dim"
            style={{ top: shown.y, height: shown.height, left: shown.x + shown.width }}
          />
        <div
          className={`selection-rect${live ? " live" : ""}`}
          data-testid="selection-rect"
          style={{ left: shown.x, top: shown.y, width: shown.width, height: shown.height }}
        >
          {live && (
            <span className="selection-size">
              {Math.round(live.width)} × {Math.round(live.height)}
            </span>
          )}
        </div>
        </>
      ) : (
        <div className="selection-dim" />
      )}
      {!drag && !selection && hint && <div className="selection-hint">{hint}</div>}
    </div>
  );
}
