import { useState, type FormEvent } from "react";
import * as halo from "../../services/halo";
import type { Diagnostics, HaloError, KeyStatus, ModelCheck, Settings as SettingsT, SettingsView } from "../../types";
import { ErrorDisplay } from "../ErrorDisplay/ErrorDisplay";

const KEY_SOURCE: Record<KeyStatus["source"], string> = {
  environment: "Using GEMINI_API_KEY from the environment.",
  credentialStore: "Stored in the system credential store.",
  none: "Not configured.",
};

interface Props {
  view: SettingsView;
  keyStatus: KeyStatus | null;
  onSaved: (view: SettingsView) => void;
  onKeyStatus: (status: KeyStatus) => void;
}

export function Settings({ view, keyStatus, onSaved, onKeyStatus }: Props) {
  const [draft, setDraft] = useState<SettingsT>(view.settings);
  const [apiKey, setApiKeyInput] = useState("");
  const [error, setError] = useState<HaloError | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [modelCheck, setModelCheck] = useState<ModelCheck | null>(null);
  const [checking, setChecking] = useState(false);
  const [diagnostics, setDiagnostics] = useState<Diagnostics | null>(null);

  const run = async (action: () => Promise<void>) => {
    setError(null);
    setNotice(null);
    try {
      await action();
    } catch (err) {
      setError(halo.toHaloError(err));
    }
  };

  const save = (e: FormEvent) => {
    e.preventDefault();
    void run(async () => {
      const saved = await halo.saveSettings(draft);
      setDraft(saved.settings);
      onSaved(saved);
      setNotice("Settings saved.");
    });
  };

  const saveKey = () =>
    run(async () => {
      onKeyStatus(await halo.setApiKey(apiKey));
      setApiKeyInput("");
      setNotice("API key saved to the system credential store.");
    });

  const clearKey = () =>
    run(async () => {
      onKeyStatus(await halo.clearApiKey());
      setNotice("Stored API key removed.");
    });

  const check = () =>
    run(async () => {
      setChecking(true);
      setModelCheck(null);
      try {
        setModelCheck(await halo.checkModel(draft.model));
      } finally {
        setChecking(false);
      }
    });

  const number = (v: string) => (v === "" ? 0 : Number(v));

  return (
    <section className="settings" aria-label="Settings">
      <h2>Settings</h2>
      {view.loadWarning && <ErrorDisplay error={{ kind: "configuration", message: view.loadWarning }} />}
      {error && <ErrorDisplay error={error} onDismiss={() => setError(null)} />}
      {notice && (
        <p className="notice" role="status">
          {notice}
        </p>
      )}

      <fieldset>
        <legend>Gemini API key</legend>
        <p data-testid="key-status">{keyStatus ? KEY_SOURCE[keyStatus.source] : "Checking…"}</p>
        {keyStatus && !keyStatus.credentialStoreAvailable && (
          <p className="muted">
            System credential store unavailable{keyStatus.credentialStoreError ? ` (${keyStatus.credentialStoreError})` : ""}.
            Set GEMINI_API_KEY in the environment instead.
          </p>
        )}
        <div className="row">
          <input
            type="password"
            autoComplete="off"
            placeholder="Paste a new API key"
            aria-label="API key"
            value={apiKey}
            onChange={(e) => setApiKeyInput(e.target.value)}
          />
          <button type="button" onClick={saveKey} disabled={!apiKey.trim()}>
            Save key
          </button>
          {keyStatus?.source === "credentialStore" && (
            <button type="button" onClick={clearKey}>
              Remove key
            </button>
          )}
        </div>
        <p className="muted">The key is kept by the native backend and is never sent to the interface or written to disk in plain text.</p>
      </fieldset>

      <form onSubmit={save}>
        <fieldset>
          <legend>Model</legend>
          <div className="row">
            <input
              aria-label="Gemini model"
              value={draft.model}
              onChange={(e) => setDraft({ ...draft, model: e.target.value.trim() })}
            />
            <button type="button" onClick={check} disabled={checking || !draft.model}>
              {checking ? "Checking…" : "Check model"}
            </button>
          </div>
          {modelCheck && (
            <p className="muted" data-testid="model-check">
              {modelCheck.displayName ?? modelCheck.model}:{" "}
              {modelCheck.supportsStreaming ? "available" : "does not support content generation"}
              {modelCheck.inputTokenLimit ? `, ${modelCheck.inputTokenLimit.toLocaleString()} input tokens` : ""}
            </p>
          )}
        </fieldset>

        <fieldset>
          <legend>Shortcut</legend>
          <input
            aria-label="Global shortcut"
            value={draft.shortcut}
            onChange={(e) => setDraft({ ...draft, shortcut: e.target.value })}
          />
          <p className="muted">Examples: Ctrl+Shift+Space, Ctrl+Alt+H, Super+F5.</p>
        </fieldset>

        <fieldset>
          <legend>Image</legend>
          <label>
            Maximum dimension (px)
            <input
              type="number"
              min={256}
              max={4096}
              step={64}
              value={draft.maxDimension}
              onChange={(e) => setDraft({ ...draft, maxDimension: number(e.target.value) })}
            />
          </label>
          <label>
            JPEG quality
            <input
              type="number"
              min={50}
              max={100}
              value={draft.jpegQuality}
              onChange={(e) => setDraft({ ...draft, jpegQuality: number(e.target.value) })}
            />
          </label>
          <label>
            Encoding
            <select
              value={draft.encoding}
              onChange={(e) => setDraft({ ...draft, encoding: e.target.value as SettingsT["encoding"] })}
            >
              <option value="auto">Auto (smallest of PNG, WebP, JPEG)</option>
              <option value="png">PNG (lossless)</option>
              <option value="webp">WebP (lossless)</option>
              <option value="jpeg">JPEG</option>
            </select>
          </label>
        </fieldset>
        <button type="submit" className="primary">
          Save settings
        </button>
      </form>

      <fieldset>
        <legend>Privacy</legend>
        <p className="muted">
          Halo captures the screen only when you activate it, keeps the capture in memory, and never saves screenshots to disk.
          The region you select and your questions are sent to Google Gemini for analysis. Model output is shown as text;
          Halo never runs code or commands from it.
        </p>
      </fieldset>

      <details
        onToggle={(e) => {
          if ((e.target as HTMLDetailsElement).open) void run(async () => setDiagnostics(await halo.getDiagnostics()));
        }}
      >
        <summary>Capture diagnostics</summary>
        {diagnostics && <pre data-testid="diagnostics">{JSON.stringify(diagnostics, null, 2)}</pre>}
      </details>
    </section>
  );
}
