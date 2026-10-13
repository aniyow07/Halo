import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { StreamEvent } from "../types";

// Isolated mock of the native backend. Production code never uses these.
type Listener = (e: StreamEvent) => void;
const asks: { question: string; emit: Listener; detached: boolean; resolve: (id: string) => void }[] = [];

vi.mock("../services/halo", () => ({
  toHaloError: (e: unknown) => e,
  getOverlaySession: vi.fn(async () => ({
    sessionId: "s1",
    frameWidth: 4,
    frameHeight: 4,
    capture: {},
    shortcut: "Ctrl+Shift+Space",
  })),
  getFrame: vi.fn(async () => new ArrayBuffer(64)),
  overlayReady: vi.fn(async () => undefined),
  dismiss: vi.fn(async () => undefined),
  processSelection: vi.fn(async () => ({
    contextId: 1,
    previewDataUrl: "data:image/png;base64,AAAA",
    stats: {
      originalWidth: 320,
      originalHeight: 160,
      processedWidth: 320,
      processedHeight: 160,
      originalBytes: 10240,
      processedBytes: 4096,
      reductionPercent: 60,
      durationMs: 4,
      format: "jpeg",
    },
  })),
  ask: vi.fn((question: string, onEvent: Listener) => {
    let resolve: (id: string) => void = () => {};
    const requestId = new Promise<string>((r) => (resolve = r));
    const entry = { question, emit: (e: StreamEvent) => !entry.detached && onEvent(e), detached: false, resolve };
    asks.push(entry);
    return { requestId, detach: () => (entry.detached = true) };
  }),
  cancelRequest: vi.fn(async () => true),
  copyText: vi.fn(async () => undefined),
  openExternal: vi.fn(async () => undefined),
}));

import * as halo from "../services/halo";
import { OverlayApp } from "./OverlayApp";

async function selectRegion() {
  render(<OverlayApp />);
  const surface = await screen.findByTestId("selection-surface");
  fireEvent.pointerDown(surface, { button: 0, clientX: 100, clientY: 100, pointerId: 1 });
  fireEvent.pointerMove(surface, { clientX: 300, clientY: 200, pointerId: 1 });
  fireEvent.pointerUp(surface, { clientX: 300, clientY: 200, pointerId: 1 });
  await screen.findByTestId("image-stats");
}

function stream(i: number, events: StreamEvent[]) {
  act(() => events.forEach((e) => asks[i].emit(e)));
}

beforeEach(() => {
  asks.length = 0;
  vi.clearAllMocks();
});

describe("overlay workflow", () => {
  it("processes a drawn selection with viewport-relative coordinates", async () => {
    await selectRegion();
    expect(halo.processSelection).toHaveBeenCalledWith(
      { x: 100, y: 100, width: 200, height: 100 },
      { width: window.innerWidth, height: window.innerHeight },
    );
    expect(screen.getByTestId("image-stats")).toHaveTextContent("320×160 JPEG · 4.0 KB");
    expect(screen.getByTestId("image-stats")).toHaveTextContent("−60%");
    expect(halo.overlayReady).toHaveBeenCalled();
  });

  it("maps against the measured surface, not a stale window.innerWidth", async () => {
    // WebKitGTK reported innerWidth 800 while the fullscreen surface was 1600 wide.
    const spy = vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockReturnValue({
      x: 0, y: 0, left: 0, top: 0, right: 1600, bottom: 1000, width: 1600, height: 1000, toJSON: () => ({}),
    } as DOMRect);
    try {
      await selectRegion();
      expect(halo.processSelection).toHaveBeenCalledWith(
        { x: 100, y: 100, width: 200, height: 100 },
        { width: 1600, height: 1000 },
      );
    } finally {
      spy.mockRestore();
    }
  });

  it("ignores accidental clicks without a drag", async () => {
    render(<OverlayApp />);
    const surface = await screen.findByTestId("selection-surface");
    fireEvent.pointerDown(surface, { button: 0, clientX: 10, clientY: 10 });
    fireEvent.pointerUp(surface, { clientX: 12, clientY: 11 });
    expect(halo.processSelection).not.toHaveBeenCalled();
  });

  it("submits a contextual action, streams, completes and copies", async () => {
    await selectRegion();
    await userEvent.click(screen.getByRole("button", { name: "Summarize" }));
    expect(asks[0].question).toMatch(/Summarize/);
    expect(screen.getByTestId("panel-status")).toHaveTextContent("Waiting for Gemini");

    stream(0, [
      { type: "started", requestId: "r1" },
      { type: "delta", requestId: "r1", seq: 1, text: "**Bold** start" },
    ]);
    expect(screen.getByTestId("panel-status")).toHaveTextContent("Gemini is answering");
    expect(screen.getByTestId("stream-cursor")).toBeInTheDocument();
    expect(screen.getByText("Bold").tagName).toBe("STRONG");

    stream(0, [
      { type: "delta", requestId: "r1", seq: 2, text: " and end." },
      { type: "done", requestId: "r1", finishReason: "STOP" },
    ]);
    expect(screen.getByTestId("panel-status")).toHaveTextContent("Done");
    expect(screen.queryByTestId("stream-cursor")).not.toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: "Copy" }));
    expect(halo.copyText).toHaveBeenCalledWith("**Bold** start and end.");
    expect(await screen.findByRole("button", { name: "Copied" })).toBeInTheDocument();
  });

  it("asks follow-up questions without reselecting", async () => {
    await selectRegion();
    await userEvent.type(screen.getByLabelText("Question"), "What is this?{Enter}");
    stream(0, [
      { type: "started", requestId: "r1" },
      { type: "delta", requestId: "r1", seq: 1, text: "A chart." },
      { type: "done", requestId: "r1", finishReason: "STOP" },
    ]);
    await userEvent.type(screen.getByLabelText("Follow-up question"), "Which bar is tallest?{Enter}");
    expect(asks).toHaveLength(2);
    expect(asks[1].question).toBe("Which bar is tallest?");
    expect(halo.processSelection).toHaveBeenCalledTimes(1);
    stream(1, [
      { type: "started", requestId: "r2" },
      { type: "delta", requestId: "r2", seq: 1, text: "The blue one." },
      { type: "done", requestId: "r2", finishReason: "STOP" },
    ]);
    expect(screen.getByText("A chart.")).toBeInTheDocument();
    expect(screen.getByText("The blue one.")).toBeInTheDocument();
  });

  it("prevents duplicate submissions while answering", async () => {
    await selectRegion();
    await userEvent.type(screen.getByLabelText("Question"), "one{Enter}");
    await userEvent.type(screen.getByLabelText("Follow-up question"), "two{Enter}");
    expect(asks).toHaveLength(1);
    expect(screen.getByRole("button", { name: "Ask" })).toBeDisabled();
  });

  it("cancels generation and retries", async () => {
    await selectRegion();
    await userEvent.type(screen.getByLabelText("Question"), "Explain{Enter}");
    stream(0, [
      { type: "started", requestId: "r1" },
      { type: "delta", requestId: "r1", seq: 1, text: "Partial" },
    ]);
    await userEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(halo.cancelRequest).toHaveBeenCalledWith("r1");
    stream(0, [{ type: "cancelled", requestId: "r1" }]);
    expect(screen.getByText("Cancelled.")).toBeInTheDocument();
    // Late output from the cancelled request must not appear.
    stream(0, [{ type: "delta", requestId: "r1", seq: 2, text: " LATE" }]);
    expect(screen.queryByText(/LATE/)).not.toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(asks).toHaveLength(2);
    expect(asks[1].question).toBe("Explain");
  });

  it("cancels a request whose id is not known yet", async () => {
    await selectRegion();
    await userEvent.type(screen.getByLabelText("Question"), "Explain{Enter}");
    await userEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(halo.cancelRequest).not.toHaveBeenCalled();
    await act(async () => asks[0].resolve("r9"));
    expect(halo.cancelRequest).toHaveBeenCalledWith("r9");
  });

  it("shows actionable errors", async () => {
    await selectRegion();
    await userEvent.type(screen.getByLabelText("Question"), "Explain{Enter}");
    stream(0, [
      { type: "started", requestId: "r1" },
      { type: "failed", requestId: "r1", error: { kind: "rateLimited", message: "Quota exceeded", retryAfterSecs: 20 } },
    ]);
    expect(screen.getByRole("alert")).toHaveTextContent("Quota exceeded");
    expect(screen.getByRole("alert")).toHaveTextContent("Try again in about 20 s.");
    expect(screen.getByRole("button", { name: "Retry" })).toBeEnabled();
  });

  it("a new selection discards the old conversation and its stream", async () => {
    await selectRegion();
    await userEvent.type(screen.getByLabelText("Question"), "Explain{Enter}");
    stream(0, [{ type: "started", requestId: "r1" }]);
    await userEvent.click(screen.getByRole("button", { name: "New selection" }));
    expect(asks[0].detached).toBe(true);
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("dismisses on Escape and from the close button", async () => {
    await selectRegion();
    fireEvent.keyDown(window, { key: "Escape" });
    expect(halo.dismiss).toHaveBeenCalledTimes(1);
    await userEvent.click(screen.getByRole("button", { name: "Dismiss Halo" }));
    expect(halo.dismiss).toHaveBeenCalledTimes(2);
  });

  it("renders model HTML as text, not markup", async () => {
    await selectRegion();
    await userEvent.type(screen.getByLabelText("Question"), "x{Enter}");
    stream(0, [
      { type: "started", requestId: "r1" },
      { type: "delta", requestId: "r1", seq: 1, text: 'Hi <img src=x onerror="alert(1)"> [link](javascript:alert(1)) [ok](https://example.com)' },
      { type: "done", requestId: "r1", finishReason: "STOP" },
    ]);
    const dialog = screen.getByRole("dialog");
    expect(dialog.querySelector("img[src='x']")).toBeNull();
    expect(screen.getByText("link").getAttribute("href") ?? "").not.toMatch(/javascript/i);
    await userEvent.click(screen.getByText("ok"));
    await waitFor(() => expect(halo.openExternal).toHaveBeenCalledWith("https://example.com"));
  });
});
