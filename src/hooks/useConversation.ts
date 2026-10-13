import { useCallback, useEffect, useReducer, useRef } from "react";
import { initialState, isBusy, reducer, retryable, type Action, type ConversationState } from "../app/conversation";
import * as halo from "../services/halo";
import type { Rect, Size } from "../types";

function newId(): string {
  return typeof crypto !== "undefined" && "randomUUID" in crypto
    ? crypto.randomUUID()
    : `${Date.now()}-${Math.random().toString(16).slice(2)}`;
}

/** Drives one overlay session: processing a selection and the streamed
 *  question/answer loop on top of it. */
export function useConversation() {
  const [state, dispatchToReact] = useReducer(reducer, initialState);
  // Synchronous mirror of the reducer state, so guards (e.g. against a
  // double-click submitting twice) see the effect of the previous action
  // before React re-renders.
  const stateRef = useRef<ConversationState>(initialState);
  const dispatch = useCallback((action: Action) => {
    stateRef.current = reducer(stateRef.current, action);
    dispatchToReact(action);
  }, []);
  // Handles of in-flight requests, detached on reset/unmount so no listener
  // outlives the request it belongs to.
  const handles = useRef(new Map<string, halo.AskHandle>());
  const requestIds = useRef(new Map<string, string>());
  const selectionToken = useRef(0);

  const detachAll = useCallback(() => {
    handles.current.forEach((h) => h.detach());
    handles.current.clear();
  }, []);

  useEffect(() => detachAll, [detachAll]);

  const startSelection = useCallback(() => {
    selectionToken.current++;
    detachAll();
    dispatch({ type: "selectionStarted" });
  }, [detachAll, dispatch]);

  const confirmSelection = useCallback(
    async (rect: Rect, viewport: Size) => {
      const token = ++selectionToken.current;
      detachAll();
      dispatch({ type: "selectionProcessing" });
      try {
        const result = await halo.processSelection(rect, viewport);
        if (token === selectionToken.current) dispatch({ type: "selectionReady", result });
      } catch (err) {
        if (token === selectionToken.current) dispatch({ type: "selectionFailed", error: halo.toHaloError(err) });
      }
    },
    [detachAll, dispatch],
  );

  const submit = useCallback((question: string) => {
    const s = stateRef.current;
    if (isBusy(s) || !s.selection || !question.trim()) return;
    const messageId = newId();
    dispatch({ type: "submit", messageId, question });
    const handle = halo.ask(question.trim(), (event) => {
      if (event.type === "started") requestIds.current.set(messageId, event.requestId);
      dispatch({ type: "stream", messageId, event });
      if (event.type !== "started" && event.type !== "delta") {
        handles.current.get(messageId)?.detach();
        handles.current.delete(messageId);
        requestIds.current.delete(messageId);
      }
    });
    handles.current.set(messageId, handle);
    handle.requestId.then(
      (requestId) => {
        requestIds.current.set(messageId, requestId);
        // Cancel was pressed before the backend had assigned an id.
        if (stateRef.current.cancelRequested && stateRef.current.activeMessageId === messageId) {
          void halo.cancelRequest(requestId);
        }
      },
      (err) => {
        handle.detach();
        handles.current.delete(messageId);
        dispatch({ type: "submitRejected", messageId, error: halo.toHaloError(err) });
      },
    );
  }, [dispatch]);

  const cancel = useCallback(() => {
    const s = stateRef.current;
    if (!s.activeMessageId) return;
    dispatch({ type: "cancelRequested", messageId: s.activeMessageId });
    const requestId = s.activeRequestId ?? requestIds.current.get(s.activeMessageId);
    if (requestId) void halo.cancelRequest(requestId);
  }, [dispatch]);

  const retry = useCallback(() => {
    const failed = retryable(stateRef.current);
    if (!failed) return;
    dispatch({ type: "removeMessage", messageId: failed.id });
    submit(failed.question);
  }, [dispatch, submit]);

  return { state, startSelection, confirmSelection, submit, cancel, retry };
}
