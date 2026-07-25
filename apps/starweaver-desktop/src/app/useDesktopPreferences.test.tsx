import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { DesktopPreferencesSnapshot } from "../bridge/types";
import { useDesktopPreferences } from "./useDesktopPreferences";

const bridgeMocks = vi.hoisted(() => ({
  getDesktopPreferences: vi.fn(),
  reloadDesktopPreferences: vi.fn(),
  updateDesktopPreferences: vi.fn(),
}));

vi.mock("../bridge/desktop", () => bridgeMocks);

const initial: DesktopPreferencesSnapshot = {
  schemaVersion: 1,
  revision: "7",
  preferences: {
    theme: "system",
    density: "comfortable",
    windowCloseBehavior: "keep_running",
  },
};

const updated: DesktopPreferencesSnapshot = {
  schemaVersion: 1,
  revision: "8",
  preferences: {
    ...initial.preferences,
    theme: "dark",
  },
};

describe("useDesktopPreferences", () => {
  beforeEach(() => {
    bridgeMocks.getDesktopPreferences.mockReset();
    bridgeMocks.getDesktopPreferences.mockResolvedValue(initial);
    bridgeMocks.reloadDesktopPreferences.mockReset();
    bridgeMocks.updateDesktopPreferences.mockReset();
  });

  it("retries an uncertain save with the exact revision and mutation identity", async () => {
    bridgeMocks.updateDesktopPreferences
      .mockRejectedValueOnce({ code: "storage", message: "Desktop preferences could not be saved" })
      .mockResolvedValueOnce(updated);
    const { result } = renderHook(() => useDesktopPreferences());
    await waitFor(() => expect(result.current.snapshot).toEqual(initial));

    await act(async () => {
      await result.current.save(updated.preferences);
    });
    expect(result.current.recoveryPending).toBe(true);
    const firstUpdate = bridgeMocks.updateDesktopPreferences.mock.calls[0]?.[0];
    expect(firstUpdate).toMatchObject({
      expectedRevision: "7",
      preferences: updated.preferences,
    });

    await act(async () => {
      await result.current.reload();
    });
    expect(bridgeMocks.reloadDesktopPreferences).not.toHaveBeenCalled();
    expect(result.current.recoveryPending).toBe(true);

    await act(async () => {
      await result.current.retryPending();
    });
    expect(bridgeMocks.updateDesktopPreferences).toHaveBeenNthCalledWith(2, firstUpdate);
    expect(result.current.snapshot).toEqual(updated);
    expect(result.current.recoveryPending).toBe(false);
  });
});
