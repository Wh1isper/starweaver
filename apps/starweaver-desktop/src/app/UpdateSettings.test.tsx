import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { UpdateSettings } from "./UpdateSettings";

const bridge = vi.hoisted(() => ({
  checkDesktopUpdate: vi.fn(),
  checkRuntimeUpdate: vi.fn(),
  getDesktopUpdateStatus: vi.fn(),
  getRuntimeUpdateStatus: vi.fn(),
  installDesktopUpdate: vi.fn(),
  installRuntimeUpdate: vi.fn(),
  rollbackRuntimeUpdate: vi.fn(),
}));

vi.mock("../bridge/desktop", () => bridge);

const desktopCurrent = {
  currentVersion: "1.2.3",
  configured: true,
};

const runtimeCurrent = {
  configured: true,
  activeVersion: "1.2.3",
  activeSource: "bundled" as const,
  selectedVersion: "1.2.3",
  selectedSource: "bundled" as const,
  restartRequired: false,
};

describe("UpdateSettings", () => {
  beforeEach(() => {
    for (const mock of Object.values(bridge)) mock.mockReset();
    bridge.getDesktopUpdateStatus.mockResolvedValue(desktopCurrent);
    bridge.getRuntimeUpdateStatus.mockResolvedValue(runtimeCurrent);
  });

  it("checks and stages only the opaque verified RPC candidate", async () => {
    bridge.checkRuntimeUpdate.mockResolvedValue({
      ...runtimeCurrent,
      candidate: {
        candidateId: "sha256:runtime-candidate",
        version: "1.2.4",
        buildRevision: "0123456789abcdef0123456789abcdef01234567",
        target: "aarch64-apple-darwin",
        size: 10 * 1024 * 1024,
      },
    });
    bridge.installRuntimeUpdate.mockResolvedValue({
      ...runtimeCurrent,
      selectedVersion: "1.2.4",
      selectedSource: "managed",
      restartRequired: true,
    });

    render(<UpdateSettings />);
    await screen.findByText("Running v1.2.3 (bundled) · next v1.2.3 (bundled)");
    fireEvent.click(screen.getByRole("button", { name: "Check RPC update" }));
    await screen.findByText(/Version 1.2.4 for aarch64-apple-darwin/);
    fireEvent.click(screen.getByRole("button", { name: "Verify and use after restart" }));

    await screen.findByText("Running v1.2.3 (bundled) · next v1.2.4 (managed)");
    await screen.findByText("Restart Starweaver to activate the selected RPC runtime.");
    expect(bridge.installRuntimeUpdate).toHaveBeenCalledWith("sha256:runtime-candidate");
  });

  it("applies only the exact backend-retained Desktop version", async () => {
    bridge.checkDesktopUpdate.mockResolvedValue({
      ...desktopCurrent,
      candidate: {
        version: "1.2.4",
        notes: "Release notes",
        platformPublisherSigned: false,
      },
    });
    bridge.installDesktopUpdate.mockResolvedValue(undefined);

    render(<UpdateSettings />);
    await screen.findByText("v1.2.3");
    fireEvent.click(screen.getByRole("button", { name: "Check Desktop update" }));
    await screen.findByText("Version 1.2.4 is available.");
    fireEvent.click(screen.getByRole("button", { name: "Install and restart" }));

    await waitFor(() => expect(bridge.installDesktopUpdate).toHaveBeenCalledWith("1.2.4"));
  });
});
