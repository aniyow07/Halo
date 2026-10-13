// Types mirrored from the Rust backend (src-tauri/src). Field names follow
// the backend's camelCase serde output.

export type ErrorKind =
  | "invalidInput"
  | "invalidSelection"
  | "capture"
  | "imageProcessing"
  | "missingApiKey"
  | "authentication"
  | "invalidModel"
  | "rateLimited"
  | "network"
  | "timeout"
  | "provider"
  | "unavailable"
  | "blocked"
  | "emptyResponse"
  | "interrupted"
  | "staleSession"
  | "busy"
  | "configuration"
  | "credentials"
  | "shortcut"
  | "window"
  | "internal";

export interface HaloError {
  kind: ErrorKind;
  message: string;
  retryAfterSecs?: number;
}

export type EncodingPreference = "auto" | "png" | "jpeg" | "webp";

export interface Settings {
  model: string;
  shortcut: string;
  maxDimension: number;
  jpegQuality: number;
  encoding: EncodingPreference;
}

export type ShortcutBackend = "globalHotkey" | "hyprlandBinding" | "unsupported";

export interface ShortcutStatus {
  shortcut: string;
  backend: ShortcutBackend;
  registered: boolean;
  message?: string;
}

export interface SettingsView {
  settings: Settings;
  shortcutStatus: ShortcutStatus | null;
  loadWarning: string | null;
}

export interface KeyStatus {
  source: "environment" | "credentialStore" | "none";
  configured: boolean;
  credentialStoreAvailable: boolean;
  credentialStoreError?: string;
}

export interface ModelCheck {
  model: string;
  displayName: string | null;
  supportsStreaming: boolean;
  inputTokenLimit: number | null;
}

export interface CaptureInfo {
  backend: string;
  monitorName: string;
  monitorX: number;
  monitorY: number;
  width: number;
  height: number;
  scaleFactor: number;
  durationMs: number;
}

export interface OverlaySession {
  sessionId: string;
  frameWidth: number;
  frameHeight: number;
  capture: CaptureInfo;
  shortcut: string;
}

export interface ProcessingStats {
  originalWidth: number;
  originalHeight: number;
  processedWidth: number;
  processedHeight: number;
  originalBytes: number;
  processedBytes: number;
  reductionPercent: number;
  durationMs: number;
  format: "png" | "jpeg" | "webp";
}

export interface SelectionResult {
  contextId: number;
  stats: ProcessingStats;
  previewDataUrl: string;
}

export interface Rect {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface Size {
  width: number;
  height: number;
}

export type StreamEvent =
  | { type: "started"; requestId: string }
  | { type: "delta"; requestId: string; seq: number; text: string }
  | { type: "done"; requestId: string; finishReason: string | null }
  | { type: "failed"; requestId: string; error: HaloError }
  | { type: "cancelled"; requestId: string };

export interface Diagnostics {
  platform: { os: string; displayServer: string; desktop: string | null; hyprland: boolean };
  captureBackend: string;
  phase: string;
  shortcut: ShortcutStatus | null;
  lastCapture: CaptureInfo | null;
  configDir: string;
  version: string;
}
