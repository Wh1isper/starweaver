import { render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { DesktopStatus } from "../bridge/types";
import App from "./App";

const bridgeMocks = vi.hoisted(() => ({
  getDesktopStatus: vi.fn<() => Promise<DesktopStatus>>(),
  onDesktopActivation: vi.fn(),
}));

vi.mock("../bridge/desktop", () => bridgeMocks);

const status: DesktopStatus = {
  appVersion: "0.9.0",
  platform: "macos",
  architecture: "aarch64",
  launchGeneration: 1,
  singleInstance: true,
  runtime: {
    state: "unavailable",
    reason: "not_configured",
  },
};

describe("App", () => {
  beforeEach(() => {
    bridgeMocks.getDesktopStatus.mockReset();
    bridgeMocks.getDesktopStatus.mockResolvedValue(status);
    bridgeMocks.onDesktopActivation.mockReset();
    bridgeMocks.onDesktopActivation.mockResolvedValue(() => undefined);
  });

  it("loads status from the privileged backend", async () => {
    render(<App />);

    expect(screen.getByText("Checking the local desktop service...")).toBeInTheDocument();
    expect(await screen.findByText("v0.9.0")).toBeInTheDocument();
    expect(screen.getByText("macos / aarch64")).toBeInTheDocument();
    expect(screen.getByText("Primary · generation 1")).toBeInTheDocument();
  });

  it("refreshes status after a secondary launch activation", async () => {
    let activationHandler: (() => void) | undefined;
    bridgeMocks.onDesktopActivation.mockImplementation(async (handler: () => void) => {
      activationHandler = handler;
      return () => undefined;
    });
    bridgeMocks.getDesktopStatus
      .mockResolvedValueOnce(status)
      .mockResolvedValueOnce({ ...status, launchGeneration: 2 });

    render(<App />);
    expect(await screen.findByText("Primary · generation 1")).toBeInTheDocument();

    activationHandler?.();

    await waitFor(() => {
      expect(screen.getByText("Primary · generation 2")).toBeInTheDocument();
    });
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
    bridgeMocks.getDesktopStatus
      .mockReturnValueOnce(initialStatus)
      .mockResolvedValueOnce({ ...status, launchGeneration: 2 });

    render(<App />);
    await waitFor(() => expect(activationHandler).toBeDefined());
    activationHandler?.();
    expect(await screen.findByText("Primary · generation 2")).toBeInTheDocument();

    resolveInitial?.(status);
    await waitFor(() => {
      expect(screen.queryByText("Primary · generation 1")).not.toBeInTheDocument();
      expect(screen.getByText("Primary · generation 2")).toBeInTheDocument();
    });
  });

  it("projects backend failures without exposing raw details", async () => {
    bridgeMocks.getDesktopStatus.mockRejectedValue(new Error("private backend path"));

    render(<App />);

    expect(
      await screen.findByText(
        "The desktop backend is unavailable. Restart Starweaver and try again.",
      ),
    ).toBeInTheDocument();
    expect(screen.queryByText(/private backend path/i)).not.toBeInTheDocument();
  });
});
