import { beforeEach, describe, expect, it, vi } from "vitest";

const { channelCallbacks, mockInvoke } = vi.hoisted(() => ({
  channelCallbacks: [] as Array<(value: unknown) => void>,
  mockInvoke: vi.fn(),
}));
const tauriApiModule = ["@tauri-apps", "api/core"].join("/");
const eventDelivery = {
  acknowledgementToken: "desktop-event-ack-v1-safe",
  event: { delivery: { record: { eventId: "event-safe" } } },
};
const readyCallback = (subscriptionIndex = 0) => channelCallbacks[subscriptionIndex * 2];
const eventCallback = (subscriptionIndex = 0) => channelCallbacks[subscriptionIndex * 2 + 1];

describe("generated Desktop host client", () => {
  beforeEach(() => {
    vi.resetModules();
    channelCallbacks.length = 0;
    mockInvoke.mockReset();
    vi.doMock(tauriApiModule, () => ({
      Channel: class {
        constructor(callback: (value: unknown) => void) {
          channelCallbacks.push(callback);
        }
      },
      invoke: mockInvoke,
    }));
  });

  it("preserves the logical invocation when an operation outcome is unresolved", async () => {
    const { DesktopHostClient, DesktopHostExecutionError } = await import(
      "../generated/host/client"
    );
    const client = new DesktopHostClient();
    const invocation = client.prepare({
      kind: "run.steer",
      input: { runId: "run-safe", sessionId: "session-safe", text: "continue" },
    });
    mockInvoke.mockRejectedValueOnce(new Error("response lost"));

    const failure = await client.execute(invocation).catch((error: unknown) => error);

    expect(failure).toBeInstanceOf(DesktopHostExecutionError);
    expect(failure).toMatchObject({ invocation });
  });

  it("retries a lost result acknowledgement without re-executing the mutation", async () => {
    const { DesktopHostAcknowledgementError, DesktopHostClient } = await import(
      "../generated/host/client"
    );
    const client = new DesktopHostClient();
    const invocation = client.prepare({
      kind: "run.steer",
      input: { runId: "run-safe", sessionId: "session-safe", text: "continue" },
    });
    const result = { accepted: true, receipt: { receiptId: "receipt-safe" } };
    mockInvoke
      .mockResolvedValueOnce({
        acknowledgementToken: "desktop-operation-ack-v1-safe",
        result,
      })
      .mockRejectedValueOnce(new Error("acknowledgement response lost"));

    const failure = await client.execute(invocation).catch((error: unknown) => error);

    expect(failure).toBeInstanceOf(DesktopHostAcknowledgementError);
    if (!(failure instanceof DesktopHostAcknowledgementError)) throw failure;
    expect(failure).toMatchObject({
      acknowledgementToken: "desktop-operation-ack-v1-safe",
      invocation,
      result,
    });
    mockInvoke.mockResolvedValueOnce(undefined);
    await expect(client.retryAcknowledgement(failure)).resolves.toEqual(result);
    expect(mockInvoke).toHaveBeenCalledTimes(3);
    expect(mockInvoke.mock.calls[0]?.[0]).toBe("execute_host_operation");
    expect(mockInvoke.mock.calls.slice(1).map((call) => call[0])).toEqual([
      "acknowledge_host_operation",
      "acknowledge_host_operation",
    ]);
  });

  it("preserves the acknowledgement handle across repeated acknowledgement failures", async () => {
    const { DesktopHostAcknowledgementError, DesktopHostClient } = await import(
      "../generated/host/client"
    );
    const client = new DesktopHostClient();
    const invocation = client.prepare({
      kind: "run.steer",
      input: { runId: "run-safe", sessionId: "session-safe", text: "continue" },
    });
    const result = { accepted: true, receipt: { receiptId: "receipt-safe" } };
    mockInvoke
      .mockResolvedValueOnce({
        acknowledgementToken: "desktop-operation-ack-v1-safe",
        result,
      })
      .mockRejectedValueOnce(new Error("first acknowledgement response lost"))
      .mockRejectedValueOnce(new Error("second acknowledgement response lost"));

    const firstFailure = await client.execute(invocation).catch((error: unknown) => error);
    if (!(firstFailure instanceof DesktopHostAcknowledgementError)) throw firstFailure;
    const secondFailure = await client
      .retryAcknowledgement(firstFailure)
      .catch((error: unknown) => error);

    expect(secondFailure).toBeInstanceOf(DesktopHostAcknowledgementError);
    if (!(secondFailure instanceof DesktopHostAcknowledgementError)) throw secondFailure;
    expect(secondFailure).toMatchObject({
      acknowledgementToken: "desktop-operation-ack-v1-safe",
      invocation,
      result,
    });
    mockInvoke.mockResolvedValueOnce(undefined);
    await expect(client.retryAcknowledgement(secondFailure)).resolves.toEqual(result);
    expect(mockInvoke.mock.calls.map((call) => call[0])).toEqual([
      "execute_host_operation",
      "acknowledge_host_operation",
      "acknowledge_host_operation",
      "acknowledge_host_operation",
    ]);
  });

  it("discovers and validates pending logical invocations through the fixed command", async () => {
    const { DesktopHostClient, LIST_PENDING_HOST_OPERATIONS_COMMAND } = await import(
      "../generated/host/client"
    );
    const pending = [
      {
        operationId: "desktop-op-v1-safe",
        operation: {
          kind: "run.steer",
          input: { runId: "run-safe", sessionId: "session-safe", text: "continue" },
        },
      },
    ];
    mockInvoke.mockResolvedValueOnce(pending);

    await expect(new DesktopHostClient().pendingOperations()).resolves.toEqual(pending);
    expect(mockInvoke).toHaveBeenCalledWith(LIST_PENDING_HOST_OPERATIONS_COMMAND);
  });

  it("surfaces handler failure, unsubscribes, and permits a fresh subscription", async () => {
    const {
      ACKNOWLEDGE_HOST_EVENT_COMMAND,
      DesktopHostClient,
      DesktopHostSubscriptionError,
      SUBSCRIBE_HOST_EVENTS_COMMAND,
      UNSUBSCRIBE_HOST_EVENTS_COMMAND,
    } = await import("../generated/host/client");
    const handlerFailure = new Error("event handler failed");
    let subscriptionCount = 0;
    mockInvoke.mockImplementation(async (command: unknown) => {
      if (command === SUBSCRIBE_HOST_EVENTS_COMMAND) {
        const subscriptionIndex = subscriptionCount;
        subscriptionCount += 1;
        const token = `desktop-host-subscription-${subscriptionCount}`;
        readyCallback(subscriptionIndex)?.(token);
        return token;
      }
      return undefined;
    });
    const client = new DesktopHostClient();
    const subscription = await client.subscribe(
      { runId: "run-safe", sessionId: "session-safe" },
      () => {
        throw handlerFailure;
      },
    );

    eventCallback()?.(eventDelivery);
    const failure = await subscription.done.catch((error: unknown) => error);

    expect(failure).toBeInstanceOf(DesktopHostSubscriptionError);
    expect(failure).toMatchObject({ cause: handlerFailure, closeCause: undefined });
    expect(mockInvoke.mock.calls.some((call) => call[0] === ACKNOWLEDGE_HOST_EVENT_COMMAND)).toBe(
      false,
    );
    expect(mockInvoke).toHaveBeenCalledWith(UNSUBSCRIBE_HOST_EVENTS_COMMAND, {
      subscriptionToken: subscription.token,
    });

    const replacement = await client.subscribe(
      { runId: "run-safe", sessionId: "session-safe" },
      () => undefined,
    );
    await replacement.close();
    await replacement.close();
    await expect(replacement.done).resolves.toBeUndefined();
    expect(
      mockInvoke.mock.calls.filter((call) => call[0] === UNSUBSCRIBE_HOST_EVENTS_COMMAND),
    ).toHaveLength(2);
  });

  it("uses the readiness token to cancel a failed replay before subscribe returns", async () => {
    const {
      DesktopHostClient,
      DesktopHostSubscriptionError,
      SUBSCRIBE_HOST_EVENTS_COMMAND,
      UNSUBSCRIBE_HOST_EVENTS_COMMAND,
    } = await import("../generated/host/client");
    const handlerFailure = new Error("early event handler failed");
    const token = "desktop-host-subscription-replay";
    let resolveSubscribe!: (token: string) => void;
    mockInvoke.mockImplementation((command: unknown) => {
      if (command === SUBSCRIBE_HOST_EVENTS_COMMAND) {
        return new Promise<string>((resolve) => {
          resolveSubscribe = resolve;
          readyCallback()?.(token);
          eventCallback()?.(eventDelivery);
        });
      }
      if (command === UNSUBSCRIBE_HOST_EVENTS_COMMAND) resolveSubscribe(token);
      return Promise.resolve(undefined);
    });
    const subscription = await new DesktopHostClient().subscribe(
      { runId: "run-safe", sessionId: "session-safe" },
      () => {
        throw handlerFailure;
      },
    );
    const failure = await subscription.done.catch((error: unknown) => error);

    expect(failure).toBeInstanceOf(DesktopHostSubscriptionError);
    expect(failure).toMatchObject({ cause: handlerFailure });
    expect(mockInvoke).toHaveBeenCalledWith(UNSUBSCRIBE_HOST_EVENTS_COMMAND, {
      subscriptionToken: token,
    });
  });

  it("returns the ready handle and reports cleanup failure while subscribe remains pending", async () => {
    const {
      DesktopHostClient,
      DesktopHostSubscriptionError,
      SUBSCRIBE_HOST_EVENTS_COMMAND,
      UNSUBSCRIBE_HOST_EVENTS_COMMAND,
    } = await import("../generated/host/client");
    const handlerFailure = new Error("setup delivery failed");
    const closeFailure = new Error("setup unsubscribe failed");
    const token = "desktop-host-subscription-pending";
    mockInvoke.mockImplementation((command: unknown) => {
      if (command === SUBSCRIBE_HOST_EVENTS_COMMAND) {
        readyCallback()?.(token);
        eventCallback()?.(eventDelivery);
        return new Promise<string>(() => undefined);
      }
      if (command === UNSUBSCRIBE_HOST_EVENTS_COMMAND) {
        return Promise.reject(closeFailure);
      }
      return Promise.resolve(undefined);
    });

    const subscription = await new DesktopHostClient().subscribe(
      { runId: "run-safe", sessionId: "session-safe" },
      () => {
        throw handlerFailure;
      },
    );
    const failure = await subscription.done.catch((error: unknown) => error);

    expect(subscription.token).toBe(token);
    expect(failure).toBeInstanceOf(DesktopHostSubscriptionError);
    expect(failure).toMatchObject({ cause: handlerFailure, closeCause: closeFailure });
  });

  it("cancels immediately and surfaces an in-flight handler failure through done", async () => {
    const {
      ACKNOWLEDGE_HOST_EVENT_COMMAND,
      DesktopHostClient,
      DesktopHostSubscriptionError,
      SUBSCRIBE_HOST_EVENTS_COMMAND,
      UNSUBSCRIBE_HOST_EVENTS_COMMAND,
    } = await import("../generated/host/client");
    const token = "desktop-host-subscription-closing";
    mockInvoke.mockImplementation(async (command: unknown) => {
      if (command === SUBSCRIBE_HOST_EVENTS_COMMAND) {
        readyCallback()?.(token);
        return token;
      }
      return undefined;
    });
    const handlerFailure = new Error("in-flight handler failed");
    let rejectHandler!: (error: Error) => void;
    const handler = vi.fn(
      () =>
        new Promise<void>((_resolve, reject) => {
          rejectHandler = reject;
        }),
    );
    const subscription = await new DesktopHostClient().subscribe(
      { runId: "run-safe", sessionId: "session-safe" },
      handler,
    );

    eventCallback()?.(eventDelivery);
    await vi.waitFor(() => expect(handler).toHaveBeenCalledTimes(1));
    await expect(subscription.close()).resolves.toBeUndefined();
    expect(
      mockInvoke.mock.calls.filter((call) => call[0] === UNSUBSCRIBE_HOST_EVENTS_COMMAND),
    ).toHaveLength(1);
    rejectHandler(handlerFailure);
    const doneFailure = await subscription.done.catch((error: unknown) => error);

    expect(doneFailure).toBeInstanceOf(DesktopHostSubscriptionError);
    expect(doneFailure).toMatchObject({ cause: handlerFailure });
    expect(mockInvoke.mock.calls.some((call) => call[0] === ACKNOWLEDGE_HOST_EVENT_COMMAND)).toBe(
      false,
    );
    expect(
      mockInvoke.mock.calls.filter((call) => call[0] === UNSUBSCRIBE_HOST_EVENTS_COMMAND),
    ).toHaveLength(1);
  });

  it("allows a handler to await close without acknowledging or self-deadlocking", async () => {
    const {
      ACKNOWLEDGE_HOST_EVENT_COMMAND,
      DesktopHostClient,
      SUBSCRIBE_HOST_EVENTS_COMMAND,
      UNSUBSCRIBE_HOST_EVENTS_COMMAND,
    } = await import("../generated/host/client");
    const token = "desktop-host-subscription-reentrant";
    let resolveSubscribe!: (token: string) => void;
    mockInvoke.mockImplementation((command: unknown) => {
      if (command === SUBSCRIBE_HOST_EVENTS_COMMAND) {
        readyCallback()?.(token);
        return new Promise<string>((resolve) => {
          resolveSubscribe = resolve;
        });
      }
      if (command === UNSUBSCRIBE_HOST_EVENTS_COMMAND) resolveSubscribe(token);
      return Promise.resolve(undefined);
    });
    let closeFromHandler: (() => Promise<void>) | undefined;
    const handler = vi.fn(async () => {
      await closeFromHandler?.();
    });
    const subscription = await new DesktopHostClient().subscribe(
      { runId: "run-safe", sessionId: "session-safe" },
      handler,
    );
    closeFromHandler = subscription.close;

    eventCallback()?.(eventDelivery);
    await expect(subscription.done).resolves.toBeUndefined();

    expect(handler).toHaveBeenCalledTimes(1);
    expect(mockInvoke.mock.calls.some((call) => call[0] === ACKNOWLEDGE_HOST_EVENT_COMMAND)).toBe(
      false,
    );
    expect(
      mockInvoke.mock.calls.filter((call) => call[0] === UNSUBSCRIBE_HOST_EVENTS_COMMAND),
    ).toHaveLength(1);
  });

  it("serializes callbacks and ignores deliveries after close", async () => {
    const {
      ACKNOWLEDGE_HOST_EVENT_COMMAND,
      DesktopHostClient,
      SUBSCRIBE_HOST_EVENTS_COMMAND,
      UNSUBSCRIBE_HOST_EVENTS_COMMAND,
    } = await import("../generated/host/client");
    const token = "desktop-host-subscription-serial";
    mockInvoke.mockImplementation(async (command: unknown) => {
      if (command === SUBSCRIBE_HOST_EVENTS_COMMAND) {
        readyCallback()?.(token);
        return token;
      }
      return undefined;
    });
    let releaseFirst!: () => void;
    const handler = vi
      .fn<() => void | Promise<void>>()
      .mockImplementationOnce(
        () =>
          new Promise<void>((resolve) => {
            releaseFirst = resolve;
          }),
      )
      .mockResolvedValueOnce(undefined);
    const subscription = await new DesktopHostClient().subscribe(
      { runId: "run-safe", sessionId: "session-safe" },
      handler,
    );

    eventCallback()?.(eventDelivery);
    eventCallback()?.(eventDelivery);
    await vi.waitFor(() => expect(handler).toHaveBeenCalledTimes(1));
    releaseFirst();
    await vi.waitFor(() => expect(handler).toHaveBeenCalledTimes(2));
    await vi.waitFor(() =>
      expect(
        mockInvoke.mock.calls.filter((call) => call[0] === ACKNOWLEDGE_HOST_EVENT_COMMAND),
      ).toHaveLength(2),
    );
    await subscription.close();
    await expect(subscription.done).resolves.toBeUndefined();

    eventCallback()?.(eventDelivery);
    await Promise.resolve();
    expect(handler).toHaveBeenCalledTimes(2);
    expect(
      mockInvoke.mock.calls.filter((call) => call[0] === ACKNOWLEDGE_HOST_EVENT_COMMAND),
    ).toHaveLength(2);
    expect(
      mockInvoke.mock.calls.filter((call) => call[0] === UNSUBSCRIBE_HOST_EVENTS_COMMAND),
    ).toHaveLength(1);
  });

  it("surfaces acknowledgement and unsubscribe failures through done", async () => {
    const {
      ACKNOWLEDGE_HOST_EVENT_COMMAND,
      DesktopHostClient,
      DesktopHostSubscriptionError,
      SUBSCRIBE_HOST_EVENTS_COMMAND,
      UNSUBSCRIBE_HOST_EVENTS_COMMAND,
    } = await import("../generated/host/client");
    const acknowledgementFailure = new Error("event acknowledgement failed");
    const closeFailure = new Error("subscription close failed");
    mockInvoke.mockImplementation(async (command: unknown) => {
      if (command === SUBSCRIBE_HOST_EVENTS_COMMAND) {
        const token = "desktop-host-subscription-safe";
        readyCallback()?.(token);
        return token;
      }
      if (command === ACKNOWLEDGE_HOST_EVENT_COMMAND) throw acknowledgementFailure;
      if (command === UNSUBSCRIBE_HOST_EVENTS_COMMAND) throw closeFailure;
      return undefined;
    });
    const handler = vi.fn();
    const subscription = await new DesktopHostClient().subscribe(
      { runId: "run-safe", sessionId: "session-safe" },
      handler,
    );

    eventCallback()?.(eventDelivery);
    const failure = await subscription.done.catch((error: unknown) => error);

    expect(handler).toHaveBeenCalledTimes(1);
    expect(failure).toBeInstanceOf(DesktopHostSubscriptionError);
    expect(failure).toMatchObject({
      cause: acknowledgementFailure,
      closeCause: closeFailure,
    });
    expect(mockInvoke).toHaveBeenCalledWith(UNSUBSCRIBE_HOST_EVENTS_COMMAND, {
      subscriptionToken: subscription.token,
    });
  });
});
