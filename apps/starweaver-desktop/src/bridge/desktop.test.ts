import { beforeEach, describe, expect, it, vi } from "vitest";
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

import { onDesktopActivation } from "./desktop";

describe("desktop bridge", () => {
  beforeEach(() => {
    coreMocks.channels.length = 0;
    coreMocks.invoke.mockReset();
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
