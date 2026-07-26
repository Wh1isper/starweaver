import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { DesktopPreferencesSnapshot, DesktopStatus } from "../bridge/types";
import App from "./App";

const bridgeMocks = vi.hoisted(() => ({
  getDesktopWindowRoute: vi.fn(),
  getDesktopStatus: vi.fn<() => Promise<DesktopStatus>>(),
  getDesktopPreferences: vi.fn<() => Promise<DesktopPreferencesSnapshot>>(),
  updateDesktopPreferences: vi.fn(),
  reloadDesktopPreferences: vi.fn(),
  onDesktopActivation: vi.fn(),
  retryManagedRuntime: vi.fn<() => Promise<void>>(),
}));

vi.mock("../bridge/desktop", () => bridgeMocks);
vi.mock("./WorkspaceProduct", () => ({
  WorkspaceProduct: () => <main>Workspace product ready</main>,
}));

const preferences: DesktopPreferencesSnapshot = {
  schemaVersion: 1,
  revision: "1",
  preferences: {
    theme: "dark",
    density: "compact",
    windowCloseBehavior: "keep_running",
  },
};

const status: DesktopStatus = {
  appVersion: "0.10.0",
  platform: "macos",
  architecture: "aarch64",
  launchGeneration: 1,
  singleInstance: true,
  runtime: {
    state: "ready",
    generation: 1,
    diagnosticsAvailable: false,
  },
};

describe("App", () => {
  beforeEach(() => {
    vi.useRealTimers();
    bridgeMocks.getDesktopWindowRoute.mockReset();
    bridgeMocks.getDesktopWindowRoute.mockResolvedValue({ kind: "main" });
    bridgeMocks.getDesktopStatus.mockReset();
    bridgeMocks.getDesktopStatus.mockResolvedValue(status);
    bridgeMocks.getDesktopPreferences.mockReset();
    bridgeMocks.getDesktopPreferences.mockResolvedValue(preferences);
    bridgeMocks.updateDesktopPreferences.mockReset();
    bridgeMocks.reloadDesktopPreferences.mockReset();
    bridgeMocks.onDesktopActivation.mockReset();
    bridgeMocks.onDesktopActivation.mockResolvedValue(() => undefined);
    bridgeMocks.retryManagedRuntime.mockReset();
    bridgeMocks.retryManagedRuntime.mockResolvedValue(undefined);
  });

  it("loads the ready local runtime and applies private appearance preferences", async () => {
    render(<App />);

    expect(screen.getByText("Opening Starweaver")).toBeInTheDocument();
    expect(await screen.findByText("Workspace product ready")).toBeInTheDocument();
    await waitFor(() => {
      expect(document.querySelector(".desktop-root")).toHaveAttribute("data-theme", "dark");
      expect(document.querySelector(".desktop-root")).toHaveAttribute("data-density", "compact");
    });
  });

  it("refreshes status after a secondary launch activation", async () => {
    let activationHandler: (() => void) | undefined;
    bridgeMocks.onDesktopActivation.mockImplementation(async (handler: () => void) => {
      activationHandler = handler;
      return () => undefined;
    });
    bridgeMocks.getDesktopStatus
      .mockResolvedValueOnce({
        ...status,
        runtime: { ...status.runtime, state: "starting" },
      })
      .mockResolvedValueOnce(status);

    render(<App />);
    expect(await screen.findByText("Waking up Starweaver")).toBeInTheDocument();

    activationHandler?.();

    expect(await screen.findByText("Workspace product ready")).toBeInTheDocument();
  });

  it("does not let an older status response overwrite a newer activation", async () => {
    let activationHandler: (() => void) | undefined;
    let resolveInitial: ((value: DesktopStatus) => void) | undefined;
    const initialStatus = new Promise<DesktopStatus>((resolve) => {
      resolveInitial = resolve;
    });
    bridgeMocks.onDesktopActivation.mockImplementation(async (handler: () => void) => {
      activationHandler = handler;
      return () => undefined;
    });
    bridgeMocks.getDesktopStatus.mockReturnValueOnce(initialStatus).mockResolvedValueOnce(status);

    render(<App />);
    await waitFor(() => expect(activationHandler).toBeDefined());
    activationHandler?.();
    expect(await screen.findByText("Workspace product ready")).toBeInTheDocument();

    resolveInitial?.({
      ...status,
      runtime: { ...status.runtime, state: "starting" },
    });
    await waitFor(() => {
      expect(screen.queryByText("Waking up Starweaver")).not.toBeInTheDocument();
      expect(screen.getByText("Workspace product ready")).toBeInTheDocument();
    });
  });

  it("offers a safe retry for failed startup", async () => {
    bridgeMocks.getDesktopStatus.mockResolvedValue({
      ...status,
      runtime: { ...status.runtime, state: "failed" },
      runtimeIssue: {
        code: "invalid_configuration",
        message: "bundled Starweaver runtime is unavailable",
      },
    });

    render(<App />);
    expect(await screen.findByText("The local runtime needs attention")).toBeInTheDocument();
    expect(screen.getByText("bundled Starweaver runtime is unavailable")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Try again" }));
    await waitFor(() => expect(bridgeMocks.retryManagedRuntime).toHaveBeenCalledOnce());
  });

  it("requires an update instead of retrying an incompatible runtime", async () => {
    bridgeMocks.getDesktopStatus.mockResolvedValue({
      ...status,
      runtime: { ...status.runtime, state: "incompatible" },
    });

    render(<App />);
    expect(await screen.findByText("This runtime cannot be used")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Try again" })).not.toBeInTheDocument();
  });

  it("loads a backend-routed conversation window without primary activation authority", async () => {
    bridgeMocks.getDesktopWindowRoute.mockResolvedValue({
      kind: "conversation",
      sessionId: "session-1",
    });

    render(<App />);

    expect(await screen.findByText("Workspace product ready")).toBeInTheDocument();
    expect(screen.getByText("Starweaver Conversation")).toBeInTheDocument();
    expect(bridgeMocks.onDesktopActivation).not.toHaveBeenCalled();
  });

  it("projects backend failures without exposing raw details", async () => {
    bridgeMocks.getDesktopStatus.mockRejectedValue(new Error("private backend path"));

    render(<App />);

    expect(await screen.findByText("Starweaver could not open")).toBeInTheDocument();
    expect(screen.queryByText(/private backend path/i)).not.toBeInTheDocument();
  });
});
