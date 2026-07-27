import { beforeEach, describe, expect, it, vi } from "vitest";
import type { DesktopHostInvocation } from "../generated/host/types";
import type { DesktopActivation } from "./types";

const coreMocks = vi.hoisted(() => {
  type MessageHandler = (message: unknown) => void;
  const channels: Array<{ onmessage: MessageHandler }> = [];

  class Channel<T> {
    onmessage: (message: T) => void;

    constructor(onmessage: (message: T) => void) {
      this.onmessage = onmessage;
      channels.push(this as { onmessage: MessageHandler });
    }
  }

  return {
    Channel,
    channels,
    invoke: vi.fn(),
  };
});

vi.mock("@tauri-apps/api/core", () => ({
  Channel: coreMocks.Channel,
  invoke: coreMocks.invoke,
}));

import {
  checkDesktopUpdate,
  checkRuntimeUpdate,
  executeDesktopWorkspaceRegistration,
  getDesktopPreferences,
  getDesktopUpdateStatus,
  getDesktopWindowRoute,
  getRuntimeUpdateStatus,
  installDesktopUpdate,
  installRuntimeUpdate,
  onDesktopActivation,
  openConversationWindow,
  reloadDesktopPreferences,
  retryManagedRuntime,
  rollbackRuntimeUpdate,
  updateDesktopPreferences,
} from "./desktop";

describe("desktop bridge", () => {
  beforeEach(() => {
    coreMocks.channels.length = 0;
    coreMocks.invoke.mockReset();
  });

  it("invokes only the fixed managed runtime retry command", async () => {
    coreMocks.invoke.mockResolvedValue(undefined);

    await retryManagedRuntime();

    expect(coreMocks.invoke).toHaveBeenCalledWith("retry_managed_runtime");
  });

  it("uses only fixed backend-owned product update commands", async () => {
    coreMocks.invoke.mockResolvedValue({});

    await getRuntimeUpdateStatus();
    await checkRuntimeUpdate();
    await installRuntimeUpdate("sha256:candidate");
    await rollbackRuntimeUpdate();
    await getDesktopUpdateStatus();
    await checkDesktopUpdate();
    await installDesktopUpdate("0.10.1");

    expect(coreMocks.invoke.mock.calls).toEqual([
      ["get_runtime_update_status"],
      ["check_runtime_update"],
      ["install_runtime_update", { candidateId: "sha256:candidate" }],
      ["rollback_runtime_update"],
      ["get_desktop_update_status"],
      ["check_desktop_update"],
      ["install_desktop_update", { version: "0.10.1" }],
    ]);
  });

  it("uses only closed Desktop preference commands and update fields", async () => {
    const snapshot = {
      schemaVersion: 1 as const,
      revision: "4",
      preferences: {
        theme: "dark" as const,
        density: "compact" as const,
        windowCloseBehavior: "keep_running" as const,
      },
    };
    coreMocks.invoke.mockResolvedValue(snapshot);

    await expect(getDesktopPreferences()).resolves.toEqual(snapshot);
    await expect(
      updateDesktopPreferences({
        expectedRevision: "4",
        mutationId: "desktop-preferences-test",
        preferences: snapshot.preferences,
      }),
    ).resolves.toEqual(snapshot);
    await expect(reloadDesktopPreferences()).resolves.toEqual(snapshot);

    expect(coreMocks.invoke.mock.calls).toEqual([
      ["get_desktop_preferences"],
      [
        "update_desktop_preferences",
        {
          update: {
            expectedRevision: "4",
            mutationId: "desktop-preferences-test",
            preferences: snapshot.preferences,
          },
        },
      ],
      ["reload_desktop_preferences"],
    ]);
  });

  it("uses fixed backend-owned conversation window routing commands", async () => {
    const route = { kind: "conversation" as const, sessionId: "session-1" };
    const opened = {
      label: "conversation-safe",
      reused: false,
      sessionId: "session-1",
    };
    coreMocks.invoke.mockResolvedValueOnce(route).mockResolvedValueOnce(opened);

    await expect(getDesktopWindowRoute()).resolves.toEqual(route);
    await expect(openConversationWindow("session-1")).resolves.toEqual(opened);

    expect(coreMocks.invoke.mock.calls).toEqual([
      ["get_desktop_window_route"],
      ["open_conversation_window", { sessionId: "session-1" }],
    ]);
  });

  it("binds a closed native workspace intent without exposing a path", async () => {
    coreMocks.invoke.mockResolvedValue({
      acknowledgementToken: "desktop-operation-ack-v1-workspace",
      result: {
        receipt: {
          operation: "workspace.register",
          receiptId: "receipt-1",
          reconciliationRequired: false,
          replayed: false,
          state: "completed",
          targetRef: "workspace-1",
        },
        workspace: {
          displayLabel: "My workspace",
          provenanceDigest: "sha256:workspace",
          revision: "1",
          state: "active",
          workspaceId: "workspace-1",
        },
      },
    });

    const invocation = {
      operationId: "desktop-op-v1-workspace",
      operation: {
        kind: "workspace.register",
        input: { displayLabel: "My workspace" },
      },
    } as DesktopHostInvocation<"workspace.register">;
    const result = await executeDesktopWorkspaceRegistration(invocation, {
      kind: "create_empty",
      name: "My workspace",
    });

    expect(result.workspace.workspaceId).toBe("workspace-1");
    expect(coreMocks.invoke).toHaveBeenCalledTimes(2);
    const [, arguments_] = coreMocks.invoke.mock.calls[0] ?? [];
    expect(arguments_).toMatchObject({
      workspaceIntent: { kind: "create_empty", name: "My workspace" },
      invocation: {
        operation: {
          kind: "workspace.register",
          input: { displayLabel: "My workspace" },
        },
      },
    });
    expect(JSON.stringify(arguments_)).not.toContain("root");
    expect(coreMocks.invoke).toHaveBeenLastCalledWith("acknowledge_host_operation", {
      acknowledgementToken: "desktop-operation-ack-v1-workspace",
    });
  });

  it("unsubscribes the exact token and fences the inactive handler", async () => {
    coreMocks.invoke.mockImplementation(async (command: string) => {
      if (command === "subscribe_desktop_activation") return 17;
      return undefined;
    });
    const handler = vi.fn();
    const stop = await onDesktopActivation(handler);
    const channel = coreMocks.channels[0];
    if (!channel) throw new Error("activation channel was not created");
    const activation: DesktopActivation = {
      kind: "secondary_launch",
      generation: 2,
    };

    channel.onmessage(activation);
    expect(handler).toHaveBeenCalledOnce();

    stop();
    channel.onmessage({ ...activation, generation: 3 });
    expect(handler).toHaveBeenCalledOnce();
    expect(coreMocks.invoke).toHaveBeenLastCalledWith("unsubscribe_desktop_activation", {
      subscriptionToken: 17,
    });
  });
});
