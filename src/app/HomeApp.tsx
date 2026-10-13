import { useEffect, useState } from "react";
import { ErrorDisplay } from "../components/ErrorDisplay/ErrorDisplay";
import { Settings } from "../components/Settings/Settings";
import * as halo from "../services/halo";
import type { HaloError, KeyStatus, SettingsView } from "../types";

function shortcutText(view: SettingsView | null): string {
  const s = view?.shortcutStatus;
  if (!view) return "";
  if (!s) return `${view.settings.shortcut} (not registered)`;
  if (!s.registered) return `${s.shortcut}: ${s.message ?? "not available"}`;
  const via = s.backend === "hyprlandBinding" ? " (Hyprland keybinding)" : "";
  return `${s.shortcut}${via}`;
}

export function HomeApp() {
  const [view, setView] = useState<SettingsView | null>(null);
  const [keyStatus, setKeyStatus] = useState<KeyStatus | null>(null);
  const [error, setError] = useState<HaloError | null>(null);
  const [activating, setActivating] = useState(false);
  const [showSettings, setShowSettings] = useState(false);

  useEffect(() => {
    Promise.all([halo.getSettings(), halo.getKeyStatus()]).then(
      ([v, k]) => {
        setView(v);
        setKeyStatus(k);
      },
      (err) => setError(halo.toHaloError(err)),
    );
    const unlisten = halo.onActivationError((e) => setError(e));
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, []);

  const activate = async () => {
    setError(null);
    setActivating(true);
    try {
      await halo.activate();
    } catch (err) {
      const e = halo.toHaloError(err);
      if (e.kind !== "busy") setError(e);
    } finally {
      setActivating(false);
    }
  };

  const shortcutOk = view?.shortcutStatus?.registered ?? false;

  return (
    <main className="home">
      <header className="home-header">
        <h1>Halo</h1>
        <p className="muted">Select anything on your screen and ask Gemini about it.</p>
      </header>

      <button type="button" className="primary activate" onClick={activate} disabled={activating}>
        {activating ? "Capturing…" : "Select screen region"}
      </button>
      <p className={`shortcut-indicator${shortcutOk ? "" : " warn"}`} data-testid="shortcut-indicator">
        Shortcut: <kbd>{shortcutText(view)}</kbd>
      </p>

      {keyStatus && !keyStatus.configured && (
        <ErrorDisplay error={{ kind: "missingApiKey", message: "No Gemini API key configured yet." }} />
      )}
      {error && <ErrorDisplay error={error} onDismiss={() => setError(null)} />}

      <div className="home-actions">
        <button type="button" onClick={() => setShowSettings((s) => !s)} aria-expanded={showSettings}>
          {showSettings ? "Hide settings" : "Settings"}
        </button>
        <button type="button" onClick={() => void halo.quit()}>
          Quit Halo
        </button>
      </div>

      {showSettings && view && (
        <Settings view={view} keyStatus={keyStatus} onSaved={setView} onKeyStatus={setKeyStatus} />
      )}
      <p className="muted small">Closing this window keeps Halo running in the background so the shortcut keeps working.</p>
    </main>
  );
}
