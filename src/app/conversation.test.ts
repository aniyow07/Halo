import { describe, expect, it } from "vitest";
import { initialState, isBusy, lastAnswer, reducer, retryable, type Action, type ConversationState } from "./conversation";
import type { SelectionResult, StreamEvent } from "../types";

const selection: SelectionResult = {
  contextId: 1,
  previewDataUrl: "data:image/png;base64,AAAA",
  stats: {
    originalWidth: 100,
    originalHeight: 50,
    processedWidth: 100,
    processedHeight: 50,
    originalBytes: 1000,
    processedBytes: 400,
    reductionPercent: 60,
    durationMs: 3,
    format: "png",
  },
};

const run = (actions: Action[], from: ConversationState = initialState) => actions.reduce(reducer, from);
const ev = (messageId: string, event: StreamEvent): Action => ({ type: "stream", messageId, event });
const ready = () => run([{ type: "selectionProcessing" }, { type: "selectionReady", result: selection }]);
const asking = (id = "m1", q = "What is this?", req = "r1") =>
  run([{ type: "submit", messageId: id, question: q }, ev(id, { type: "started", requestId: req })], ready());

describe("selection lifecycle", () => {
  it("goes selecting -> processing -> ready", () => {
    expect(initialState.phase).toBe("selectingRegion");
    const p = reducer(initialState, { type: "selectionProcessing" });
    expect(p.phase).toBe("processingImage");
    expect(isBusy(p)).toBe(true);
    const r = reducer(p, { type: "selectionReady", result: selection });
    expect(r.phase).toBe("ready");
    expect(r.selection).toEqual(selection);
  });

  it("reports processing failures and returns to selecting", () => {
    const s = run([{ type: "selectionProcessing" }, { type: "selectionFailed", error: { kind: "invalidSelection", message: "too small" } }]);
    expect(s.phase).toBe("selectingRegion");
    expect(s.selectionError?.message).toBe("too small");
  });

  it("ignores a late result for an abandoned selection", () => {
    const s = run([{ type: "selectionStarted" }, { type: "selectionReady", result: selection }]);
    expect(s.selection).toBeNull();
  });

  it("a new selection resets the conversation", () => {
    const s = run([ev("m1", { type: "delta", requestId: "r1", seq: 1, text: "hi" }), { type: "selectionStarted" }], asking());
    expect(s.messages).toEqual([]);
    expect(s.activeRequestId).toBeNull();
  });
});

describe("questions and streaming", () => {
  it("rejects submissions without a selection or while busy", () => {
    expect(reducer(initialState, { type: "submit", messageId: "x", question: "q" })).toBe(initialState);
    const s = asking();
    expect(reducer(s, { type: "submit", messageId: "m2", question: "again" })).toBe(s);
    expect(reducer(ready(), { type: "submit", messageId: "m", question: "   " }).messages).toHaveLength(0);
  });

  it("streams deltas and completes", () => {
    let s = asking();
    expect(s.phase).toBe("waitingForModel");
    s = run([ev("m1", { type: "delta", requestId: "r1", seq: 1, text: "Hel" }), ev("m1", { type: "delta", requestId: "r1", seq: 2, text: "lo" })], s);
    expect(s.phase).toBe("streamingResponse");
    expect(s.messages[0].answer).toBe("Hello");
    expect(s.messages[0].status).toBe("streaming");
    s = reducer(s, ev("m1", { type: "done", requestId: "r1", finishReason: "STOP" }));
    expect(s.phase).toBe("completed");
    expect(s.messages[0].status).toBe("done");
    expect(isBusy(s)).toBe(false);
    expect(lastAnswer(s)).toBe("Hello");
  });

  it("drops duplicate and out-of-order deltas", () => {
    const s = run(
      [
        ev("m1", { type: "delta", requestId: "r1", seq: 1, text: "a" }),
        ev("m1", { type: "delta", requestId: "r1", seq: 1, text: "a" }),
        ev("m1", { type: "delta", requestId: "r1", seq: 3, text: "c" }),
        ev("m1", { type: "delta", requestId: "r1", seq: 2, text: "b" }),
      ],
      asking(),
    );
    expect(s.messages[0].answer).toBe("ac");
  });

  it("ignores events from other requests or messages", () => {
    const s0 = asking();
    const s = run(
      [
        ev("m1", { type: "delta", requestId: "other", seq: 1, text: "x" }),
        ev("old", { type: "delta", requestId: "r1", seq: 1, text: "y" }),
        ev("m1", { type: "started", requestId: "r2" }),
        ev("m1", { type: "done", requestId: "other", finishReason: "STOP" }),
      ],
      s0,
    );
    expect(s.messages[0].answer).toBe("");
    expect(s.activeRequestId).toBe("r1");
    expect(s.phase).toBe("waitingForModel");
  });

  it("supports follow-up questions after completion", () => {
    let s = run([ev("m1", { type: "delta", requestId: "r1", seq: 1, text: "first" }), ev("m1", { type: "done", requestId: "r1", finishReason: "STOP" })], asking());
    s = run([{ type: "submit", messageId: "m2", question: "and then?" }, ev("m2", { type: "started", requestId: "r2" }), ev("m2", { type: "delta", requestId: "r2", seq: 1, text: "second" }), ev("m2", { type: "done", requestId: "r2", finishReason: "STOP" })], s);
    expect(s.messages.map((m) => [m.question, m.answer])).toEqual([
      ["What is this?", "first"],
      ["and then?", "second"],
    ]);
  });
});

describe("cancellation, errors and retry", () => {
  it("marks cancellation and ignores events after it", () => {
    let s = reducer(asking(), { type: "cancelRequested", messageId: "m1" });
    expect(s.cancelRequested).toBe(true);
    s = reducer(s, ev("m1", { type: "cancelled", requestId: "r1" }));
    expect(s.phase).toBe("cancelled");
    expect(s.messages[0].status).toBe("cancelled");
    const after = reducer(s, ev("m1", { type: "delta", requestId: "r1", seq: 5, text: "late" }));
    expect(after).toBe(s);
    expect(retryable(s)?.question).toBe("What is this?");
  });

  it("shows provider failures and allows retry", () => {
    const s = reducer(asking(), ev("m1", { type: "failed", requestId: "r1", error: { kind: "rateLimited", message: "quota", retryAfterSecs: 5 } }));
    expect(s.phase).toBe("error");
    expect(s.messages[0].error?.kind).toBe("rateLimited");
    expect(retryable(s)?.id).toBe("m1");
    const removed = reducer(s, { type: "removeMessage", messageId: "m1" });
    expect(removed.messages).toHaveLength(0);
  });

  it("handles rejection before the request starts", () => {
    const s = run([{ type: "submit", messageId: "m1", question: "q" }, { type: "submitRejected", messageId: "m1", error: { kind: "missingApiKey", message: "no key" } }], ready());
    expect(s.messages[0].status).toBe("error");
    expect(isBusy(s)).toBe(false);
  });

  it("does not retry while busy", () => {
    expect(retryable(asking())).toBeNull();
  });
});
