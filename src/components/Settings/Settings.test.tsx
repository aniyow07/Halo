import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { SettingsView } from "../../types";

vi.mock("../../services/halo", () => ({
  toHaloError: (e: unknown) => e,
  saveSettings: vi.fn(async (settings) => ({ settings, shortcutStatus: null, loadWarning: null })),
  setApiKey: vi.fn(async () => ({ source: "credentialStore", configured: true, credentialStoreAvailable: true })),
  clearApiKey: vi.fn(async () => ({ source: "none", configured: false, credentialStoreAvailable: true })),
  checkModel: vi.fn(async () => ({ model: "gemini-3.8-flash", displayName: "Gemini 3.8 Flash", supportsStreaming: true, inputTokenLimit: 1048576 })),
  getDiagnostics: vi.fn(),
}));

import * as halo from "../../services/halo";
import { Settings } from "./Settings";

const view: SettingsView = {
  settings: { model: "gemini-3.8-flash", shortcut: "Ctrl+Shift+Space", maxDimension: 2048, jpegQuality: 90, encoding: "auto" },
  shortcutStatus: { shortcut: "Ctrl+Shift+Space", backend: "hyprlandBinding", registered: true },
  loadWarning: null,
};

beforeEach(() => vi.clearAllMocks());

describe("Settings", () => {
  it("saves edited settings to the backend", async () => {
    const onSaved = vi.fn();
    render(<Settings view={view} keyStatus={null} onSaved={onSaved} onKeyStatus={vi.fn()} />);
    const dim = screen.getByLabelText("Maximum dimension (px)");
    await userEvent.clear(dim);
    await userEvent.type(dim, "1024");
    await userEvent.selectOptions(screen.getByLabelText("Encoding"), "jpeg");
    await userEvent.clear(screen.getByLabelText("Global shortcut"));
    await userEvent.type(screen.getByLabelText("Global shortcut"), "Ctrl+Alt+H");
    await userEvent.click(screen.getByRole("button", { name: "Save settings" }));
    expect(halo.saveSettings).toHaveBeenCalledWith({ ...view.settings, maxDimension: 1024, encoding: "jpeg", shortcut: "Ctrl+Alt+H" });
    expect(onSaved).toHaveBeenCalled();
    expect(await screen.findByRole("status")).toHaveTextContent("Settings saved.");
  });

  it("shows backend validation errors", async () => {
    vi.mocked(halo.saveSettings).mockRejectedValueOnce({ kind: "configuration", message: "Maximum image dimension must be between 256 and 4096 px." });
    render(<Settings view={view} keyStatus={null} onSaved={vi.fn()} onKeyStatus={vi.fn()} />);
    await userEvent.click(screen.getByRole("button", { name: "Save settings" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("between 256 and 4096");
  });

  it("stores an API key without echoing it", async () => {
    const onKeyStatus = vi.fn();
    render(
      <Settings
        view={view}
        keyStatus={{ source: "none", configured: false, credentialStoreAvailable: true }}
        onSaved={vi.fn()}
        onKeyStatus={onKeyStatus}
      />,
    );
    expect(screen.getByTestId("key-status")).toHaveTextContent("Not configured");
    const input = screen.getByLabelText("API key");
    expect(input).toHaveAttribute("type", "password");
    await userEvent.type(input, "AIzaSyTestKey000000000000000");
    await userEvent.click(screen.getByRole("button", { name: "Save key" }));
    expect(halo.setApiKey).toHaveBeenCalledWith("AIzaSyTestKey000000000000000");
    expect(onKeyStatus).toHaveBeenCalledWith(expect.objectContaining({ source: "credentialStore" }));
    expect(input).toHaveValue("");
  });

  it("checks the model against the live API", async () => {
    render(<Settings view={view} keyStatus={null} onSaved={vi.fn()} onKeyStatus={vi.fn()} />);
    await userEvent.click(screen.getByRole("button", { name: "Check model" }));
    expect(halo.checkModel).toHaveBeenCalledWith("gemini-3.8-flash");
    expect(await screen.findByTestId("model-check")).toHaveTextContent("Gemini 3.8 Flash: available");
  });
});
