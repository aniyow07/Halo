import { forwardRef, useEffect, useRef, useState, type CSSProperties, type FormEvent, type KeyboardEvent } from "react";
import { isBusy, lastAnswer, retryable, type ConversationState } from "../../app/conversation";
import { ChatMessages } from "../ChatMessages/ChatMessages";
import { ContextualActions } from "../ContextualActions/ContextualActions";
import { ErrorDisplay } from "../ErrorDisplay/ErrorDisplay";
import { ImagePreview } from "../ImagePreview/ImagePreview";

interface Props {
  state: ConversationState;
  style?: CSSProperties;
  onSubmit: (question: string) => void;
  onCancel: () => void;
  onRetry: () => void;
  onCopy: (text: string) => Promise<void>;
  onNewSelection: () => void;
  onDismiss: () => void;
}

const PHASE_LABEL: Record<ConversationState["phase"], string> = {
  selectingRegion: "Select a region",
  processingImage: "Processing image…",
  ready: "Ask about the selection",
  waitingForModel: "Waiting for Gemini…",
  streamingResponse: "Gemini is answering…",
  completed: "Done",
  error: "Something went wrong",
  cancelled: "Cancelled",
};

export const FloatingHaloPanel = forwardRef<HTMLDivElement, Props>(function FloatingHaloPanel(
  { state, style, onSubmit, onCancel, onRetry, onCopy, onNewSelection, onDismiss },
  ref,
) {
  const [question, setQuestion] = useState("");
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const ready = state.selection !== null;
  // The textarea is disabled until the image is processed, so `autoFocus`
  // alone would never focus it. Focus it as soon as questions are possible
  // and again after each answer, so follow-ups can be typed directly.
  useEffect(() => {
    if (ready && !isBusy(state)) inputRef.current?.focus();
  }, [ready, state]);
  const [copyState, setCopyState] = useState<"idle" | "copied" | "failed">("idle");
  const busy = isBusy(state);
  const canAsk = !busy && state.selection !== null;
  const answer = lastAnswer(state);
  const failed = retryable(state);
  const streaming = state.phase === "waitingForModel" || state.phase === "streamingResponse";

  const send = (text: string) => {
    if (!canAsk || !text.trim()) return;
    onSubmit(text);
    setQuestion("");
  };
  const onFormSubmit = (e: FormEvent) => {
    e.preventDefault();
    send(question);
  };
  const onKeyDown = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      send(question);
    }
  };
  const copy = async () => {
    if (!answer) return;
    try {
      await onCopy(answer);
      setCopyState("copied");
    } catch {
      setCopyState("failed");
    }
    setTimeout(() => setCopyState("idle"), 1500);
  };

  return (
    <div
      ref={ref}
      className="halo-panel"
      style={style}
      role="dialog"
      aria-label="Halo"
      onPointerDown={(e) => e.stopPropagation()}
    >
      <header className="panel-header">
        <span className="panel-status" data-testid="panel-status">
          {streaming && <span className="spinner" aria-hidden />}
          {PHASE_LABEL[state.phase]}
        </span>
        <button type="button" className="icon-button" onClick={onDismiss} aria-label="Dismiss Halo" title="Dismiss (Esc)">
          ✕
        </button>
      </header>

      <ImagePreview
        src={state.selection?.previewDataUrl ?? null}
        stats={state.selection?.stats ?? null}
        processing={state.phase === "processingImage"}
      />

      {state.selectionError && <ErrorDisplay error={state.selectionError} />}

      <ChatMessages messages={state.messages} />

      {state.messages.length === 0 && <ContextualActions disabled={!canAsk} onAction={send} />}

      <form className="question-form" onSubmit={onFormSubmit}>
        <textarea
          value={question}
          onChange={(e) => setQuestion(e.target.value)}
          onKeyDown={onKeyDown}
          placeholder={state.messages.length ? "Ask a follow-up…" : "Ask about the selection…"}
          aria-label={state.messages.length ? "Follow-up question" : "Question"}
          ref={inputRef}
          rows={2}
          maxLength={8000}
          disabled={state.selection === null}
        />
        <button type="submit" disabled={!canAsk || !question.trim()}>
          Ask
        </button>
      </form>

      <div className="panel-toolbar">
        {busy && state.phase !== "processingImage" && (
          <button type="button" onClick={onCancel} disabled={state.cancelRequested}>
            {state.cancelRequested ? "Cancelling…" : "Cancel"}
          </button>
        )}
        {failed && (
          <button type="button" onClick={onRetry}>
            Retry
          </button>
        )}
        <button type="button" onClick={copy} disabled={!answer || busy}>
          {copyState === "copied" ? "Copied" : copyState === "failed" ? "Copy failed" : "Copy"}
        </button>
        <button type="button" onClick={onNewSelection}>
          New selection
        </button>
      </div>
      <p className="privacy-note">The selected image and your questions are sent to Google Gemini.</p>
    </div>
  );
});
