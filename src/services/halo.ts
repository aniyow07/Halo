// Thin, typed wrappers around the Rust commands. Components only talk to
// the backend through this module, so tests can mock it as a unit.

import { Channel, invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  Diagnostics,
  HaloError,
  KeyStatus,
  ModelCheck,
  OverlaySession,
  Rect,
  SelectionResult,
  Settings,
  SettingsView,
  Size,
  StreamEvent,
} from "../types";

/** Normalise anything thrown by `invoke` into a HaloError. */
export function toHaloError(err: unknown): HaloError {
  if (err && typeof err === "object" && "kind" in err && "message" in err) {
    return err as HaloError;
  }
  return { kind: "internal", message: err instanceof Error ? err.message : String(err) };
}

async function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(cmd, args);
  } catch (err) {
    throw toHaloError(err);
  }
}

export const activate = () => call<null>("activate");
export const getOverlaySession = () => call<OverlaySession>("get_overlay_session");
export const overlayReady = () => call<null>("overlay_ready");
export const dismiss = () => call<null>("dismiss");
export const processSelection = (rect: Rect, viewport: Size) =>
  call<SelectionResult>("process_selection", { rect, viewport });

/** Raw RGBA bytes of the frozen frame. */
export async function getFrame(): Promise<ArrayBuffer> {
  const data = await call<ArrayBuffer | number[]>("get_frame");
  return data instanceof ArrayBuffer ? data : new Uint8Array(data).buffer;
}

export interface AskHandle {
  /** Resolves with the backend request id once the request is accepted. */
  requestId: Promise<string>;
  /** Stop delivering events (the backend request keeps its own lifecycle). */
  detach: () => void;
}

/** Start a streaming question. Events arrive on a dedicated channel, so
 *  events from different requests can never interleave on one listener. */
export function ask(question: string, onEvent: (event: StreamEvent) => void): AskHandle {
  let attached = true;
  const channel = new Channel<StreamEvent>();
  channel.onmessage = (event) => {
    if (attached) onEvent(event);
  };
  return {
    requestId: call<string>("ask", { question, onEvent: channel }),
    detach: () => {
      attached = false;
    },
  };
}

export const cancelRequest = (requestId: string) => call<boolean>("cancel_request", { requestId });
export const getSettings = () => call<SettingsView>("get_settings");
export const saveSettings = (settings: Settings) => call<SettingsView>("save_settings", { settings });
export const getKeyStatus = () => call<KeyStatus>("get_key_status");
export const setApiKey = (key: string) => call<KeyStatus>("set_api_key", { key });
export const clearApiKey = () => call<KeyStatus>("clear_api_key");
export const checkModel = (model: string) => call<ModelCheck>("check_model", { model });
export const getDiagnostics = () => call<Diagnostics>("get_diagnostics");
export const openExternal = (url: string) => call<null>("open_external", { url });
export const showMain = () => call<null>("show_main");
export const quit = () => call<null>("quit");

export function onActivationError(handler: (error: HaloError) => void): Promise<UnlistenFn> {
  return listen<HaloError>("halo://activation-error", (e) => handler(e.payload));
}

/** Copy text, falling back to execCommand where the async clipboard API is
 *  unavailable (some WebKitGTK builds). */
export async function copyText(text: string): Promise<void> {
  try {
    if (navigator.clipboard?.writeText) {
      await navigator.clipboard.writeText(text);
      return;
    }
  } catch {
    // fall through to the legacy path
  }
  const area = document.createElement("textarea");
  area.value = text;
  area.setAttribute("readonly", "");
  area.style.position = "fixed";
  area.style.opacity = "0";
  document.body.appendChild(area);
  area.select();
  const ok = document.execCommand("copy");
  area.remove();
  if (!ok) throw new Error("Clipboard is not available.");
}
