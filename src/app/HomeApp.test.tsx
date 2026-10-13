import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

vi.mock("../services/halo", () => ({
  toHaloError: (e: unknown) => e,
  getSettings: vi.fn(async () => ({
    settings: { model: "gemini-3.8-flash", shortcut: "Ctrl+Shift+Space", maxDimension: 2048, jpegQuality: 90, encoding: "auto" },
    shortcutStatus: { shortcut: "Ctrl+Shift+Space", backend: "hyprlandBinding", registered: true },
    loadWarning: null,
  })),
  getKeyStatus: vi.fn(async () => ({ source: "none", configured: false, credentialStoreAvailable: true })),
  onActivationError: vi.fn(async () => () => {}),
  activate: vi.fn(async () => {
    throw { kind: "capture", message: "Screen capture failed on eDP-1" };
  }),
  quit: vi.fn(),
}));

import * as halo from "../services/halo";
import { HomeApp } from "./HomeApp";

describe("HomeApp", () => {
  it("shows the shortcut, key status and activation errors", async () => {
    render(<HomeApp />);
    expect(await screen.findByTestId("shortcut-indicator")).toHaveTextContent("Ctrl+Shift+Space (Hyprland keybinding)");
    expect(screen.getByText("No Gemini API key configured yet.")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Select screen region" }));
    expect(halo.activate).toHaveBeenCalled();
    expect(await screen.findByText("Screen capture failed on eDP-1")).toBeInTheDocument();
  });
});
