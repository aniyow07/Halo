// Pure state machine for one overlay session: selection -> image ->
// questions/answers. Every stream event carries the id of the message it
// belongs to and the backend request id; anything that does not match the
// active request is dropped, so stale, duplicate or post-cancel events can
// never change what is on screen.

import type { HaloError, SelectionResult, StreamEvent } from "../types";

export type Phase =
  | "selectingRegion"
  | "processingImage"
  | "ready"
  | "waitingForModel"
  | "streamingResponse"
  | "completed"
  | "error"
  | "cancelled";

export type MessageStatus = "pending" | "streaming" | "done" | "error" | "cancelled";

export interface Message {
  id: string;
  question: string;
  answer: string;
  status: MessageStatus;
  error?: HaloError;
  finishReason?: string | null;
}

export interface ConversationState {
  phase: Phase;
  selection: SelectionResult | null;
  selectionError: HaloError | null;
  messages: Message[];
  activeMessageId: string | null;
  activeRequestId: string | null;
  lastSeq: number;
  cancelRequested: boolean;
}

export type Action =
  | { type: "selectionStarted" }
  | { type: "selectionProcessing" }
  | { type: "selectionReady"; result: SelectionResult }
  | { type: "selectionFailed"; error: HaloError }
  | { type: "submit"; messageId: string; question: string }
  | { type: "submitRejected"; messageId: string; error: HaloError }
  | { type: "cancelRequested"; messageId: string }
  | { type: "stream"; messageId: string; event: StreamEvent }
  | { type: "removeMessage"; messageId: string };

export const initialState: ConversationState = {
  phase: "selectingRegion",
  selection: null,
  selectionError: null,
  messages: [],
  activeMessageId: null,
  activeRequestId: null,
  lastSeq: 0,
  cancelRequested: false,
};

export function isBusy(state: ConversationState): boolean {
  return state.activeMessageId !== null || state.phase === "processingImage";
}

function updateMessage(state: ConversationState, id: string, patch: Partial<Message>): Message[] {
  return state.messages.map((m) => (m.id === id ? { ...m, ...patch } : m));
}

function settle(state: ConversationState, phase: Phase, patch: Partial<Message>): ConversationState {
  return {
    ...state,
    phase,
    messages: updateMessage(state, state.activeMessageId as string, patch),
    activeMessageId: null,
    activeRequestId: null,
    lastSeq: 0,
    cancelRequested: false,
  };
}

export function reducer(state: ConversationState, action: Action): ConversationState {
  switch (action.type) {
    case "selectionStarted":
      // A new selection is a new visual context: everything else resets.
      return { ...initialState };
    case "selectionProcessing":
      return { ...initialState, phase: "processingImage" };
    case "selectionReady":
      if (state.phase !== "processingImage") return state;
      return { ...state, phase: "ready", selection: action.result, selectionError: null };
    case "selectionFailed":
      if (state.phase !== "processingImage") return state;
      return { ...initialState, phase: "selectingRegion", selectionError: action.error };
    case "submit": {
      if (isBusy(state) || !state.selection) return state;
      const question = action.question.trim();
      if (!question) return state;
      return {
        ...state,
        phase: "waitingForModel",
        messages: [...state.messages, { id: action.messageId, question, answer: "", status: "pending" }],
        activeMessageId: action.messageId,
        activeRequestId: null,
        lastSeq: 0,
        cancelRequested: false,
      };
    }
    case "submitRejected":
      if (action.messageId !== state.activeMessageId) return state;
      return settle(state, "error", { status: "error", error: action.error });
    case "cancelRequested":
      if (action.messageId !== state.activeMessageId) return state;
      return { ...state, cancelRequested: true };
    case "removeMessage":
      if (action.messageId === state.activeMessageId) return state;
      return { ...state, messages: state.messages.filter((m) => m.id !== action.messageId) };
    case "stream":
      return applyStreamEvent(state, action.messageId, action.event);
  }
}

function applyStreamEvent(state: ConversationState, messageId: string, event: StreamEvent): ConversationState {
  if (messageId !== state.activeMessageId) return state;
  if (event.type === "started") {
    if (state.activeRequestId !== null) return state;
    return { ...state, activeRequestId: event.requestId };
  }
  if (event.requestId !== state.activeRequestId) return state;

  const current = state.messages.find((m) => m.id === messageId);
  if (!current) return state;

  switch (event.type) {
    case "delta":
      // Channels are ordered, but never trust it: drop duplicates and
      // anything older than what has been rendered.
      if (event.seq <= state.lastSeq) return state;
      return {
        ...state,
        phase: "streamingResponse",
        lastSeq: event.seq,
        messages: updateMessage(state, messageId, { answer: current.answer + event.text, status: "streaming" }),
      };
    case "done":
      return settle(state, "completed", { status: "done", finishReason: event.finishReason });
    case "failed":
      return settle(state, "error", { status: "error", error: event.error });
    case "cancelled":
      return settle(state, "cancelled", { status: "cancelled" });
  }
}

/** The last message, if it can be retried (failed or cancelled). */
export function retryable(state: ConversationState): Message | null {
  const last = state.messages[state.messages.length - 1];
  if (!last || isBusy(state)) return null;
  return last.status === "error" || last.status === "cancelled" ? last : null;
}

/** The most recent completed answer, for copying. */
export function lastAnswer(state: ConversationState): string | null {
  for (let i = state.messages.length - 1; i >= 0; i--) {
    const m = state.messages[i];
    if (m.answer && (m.status === "done" || m.status === "cancelled" || m.status === "error")) return m.answer;
  }
  return null;
}
