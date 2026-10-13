import type { HaloError } from "../../types";

const HINTS: Partial<Record<HaloError["kind"], string>> = {
  missingApiKey: "Open Halo's settings to add a Gemini API key.",
  authentication: "Check the API key in Halo's settings.",
  invalidModel: "Pick a different model in Halo's settings.",
  network: "Check your internet connection, then retry.",
  timeout: "Retry; Gemini may be busy.",
  unavailable: "Gemini is overloaded right now. Retry in a moment.",
  interrupted: "Retry to get a complete answer.",
  capture: "See Diagnostics in Halo's settings for the capture backend.",
};

export function errorHint(error: HaloError): string | undefined {
  if (error.kind === "rateLimited") {
    return error.retryAfterSecs ? `Try again in about ${error.retryAfterSecs} s.` : "Wait a moment, then retry.";
  }
  return HINTS[error.kind];
}

export function ErrorDisplay({ error, onDismiss }: { error: HaloError; onDismiss?: () => void }) {
  const hint = errorHint(error);
  return (
    <div className="error-display" role="alert">
      <strong>{error.message}</strong>
      {hint && <span className="error-hint"> {hint}</span>}
      {onDismiss && (
        <button type="button" className="link-button" onClick={onDismiss}>
          Dismiss
        </button>
      )}
    </div>
  );
}
