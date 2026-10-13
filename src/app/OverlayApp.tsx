import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { FloatingHaloPanel } from "../components/FloatingHaloPanel/FloatingHaloPanel";
import { ErrorDisplay } from "../components/ErrorDisplay/ErrorDisplay";
import { SelectionOverlay, type FrozenFrame } from "../components/SelectionOverlay/SelectionOverlay";
import { useConversation } from "../hooks/useConversation";
import * as halo from "../services/halo";
import type { HaloError, Rect, Size } from "../types";
import { computePanelPosition } from "./panelPosition";

const PANEL_FALLBACK = { width: 420, height: 520 };

/** The full-screen selection overlay and its floating answer panel. */
export function OverlayApp() {
  const [frame, setFrame] = useState<FrozenFrame | null>(null);
  const [loadError, setLoadError] = useState<HaloError | null>(null);
  const [selection, setSelection] = useState<Rect | null>(null);
  const [viewport, setViewport] = useState<Size | null>(null);
  const [panelPos, setPanelPos] = useState<{ left: number; top: number } | null>(null);
  const panelRef = useRef<HTMLDivElement>(null);
  const readySent = useRef(false);
  const { state, startSelection, confirmSelection, submit, cancel, retry } = useConversation();

  useEffect(() => {
    let alive = true;
    (async () => {
      try {
        const session = await halo.getOverlaySession();
        const rgba = await halo.getFrame();
        if (alive) setFrame({ width: session.frameWidth, height: session.frameHeight, rgba });
      } catch (err) {
        if (!alive) return;
        setLoadError(halo.toHaloError(err));
        void halo.overlayReady();
      }
    })();
    return () => {
      alive = false;
    };
  }, []);

  const onFramePainted = useCallback(() => {
    if (readySent.current) return;
    readySent.current = true;
    void halo.overlayReady();
  }, []);

  const dismiss = useCallback(() => {
    cancel();
    void halo.dismiss();
  }, [cancel]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") dismiss();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [dismiss]);

  const onSelect = (rect: Rect, measured: Size) => {
    setSelection(rect);
    setViewport(measured);
    void confirmSelection(rect, measured);
  };

  const newSelection = () => {
    setSelection(null);
    setPanelPos(null);
    startSelection();
  };

  // Keep the panel inside the screen as its height changes while streaming.
  useLayoutEffect(() => {
    if (!selection) return;
    const el = panelRef.current;
    const size = el ? { width: el.offsetWidth, height: el.offsetHeight } : PANEL_FALLBACK;
    const bounds = viewport ?? { width: window.innerWidth, height: window.innerHeight };
    const next = computePanelPosition(selection, size, bounds);
    if (!panelPos || next.left !== panelPos.left || next.top !== panelPos.top) setPanelPos(next);
  }, [selection, viewport, state, panelPos]);

  if (loadError) {
    return (
      <div className="overlay-error">
        <ErrorDisplay error={loadError} />
        <button type="button" onClick={dismiss}>
          Close
        </button>
      </div>
    );
  }

  return (
    <>
      <SelectionOverlay
        frame={frame}
        selection={selection}
        onSelect={onSelect}
        onFramePainted={onFramePainted}
        hint={state.selectionError ? state.selectionError.message : "Drag to select a region · Esc to cancel"}
      />
      {selection && (
        <FloatingHaloPanel
          ref={panelRef}
          state={state}
          style={panelPos ? { left: panelPos.left, top: panelPos.top } : { visibility: "hidden" }}
          onSubmit={submit}
          onCancel={cancel}
          onRetry={retry}
          onCopy={halo.copyText}
          onNewSelection={newSelection}
          onDismiss={dismiss}
        />
      )}
    </>
  );
}
