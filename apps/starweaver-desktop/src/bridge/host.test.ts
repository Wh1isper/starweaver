import { describe, expect, it } from "vitest";
import {
  DesktopHostValidationError,
  parseDesktopHostEventDelivery,
  parseDesktopHostOperationDelivery,
  parseDesktopHostResult,
  parsePendingDesktopHostOperations,
} from "../generated/host/validators";

describe("generated Desktop host projection", () => {
  it("accepts backend page tokens without exposing host cursors", () => {
    expect(
      parseDesktopHostResult("session.list", {
        page: { hasMore: true, nextPageToken: "desktop-page-safe" },
        sessions: [],
      }),
    ).toEqual({
      page: { hasMore: true, nextPageToken: "desktop-page-safe" },
      sessions: [],
    });

    expect(() =>
      parseDesktopHostResult("session.list", {
        page: { hasMore: true, nextCursor: "host-private-cursor" },
        sessions: [],
      }),
    ).toThrow(DesktopHostValidationError);
  });

  it("validates pending operation handles without admitting supervisor authority", () => {
    expect(
      parsePendingDesktopHostOperations([
        {
          operationId: "desktop-op-v1-safe",
          operation: {
            kind: "run.steer",
            input: { runId: "run-safe", sessionId: "session-safe", text: "continue" },
          },
        },
      ]),
    ).toHaveLength(1);

    expect(() =>
      parsePendingDesktopHostOperations([
        {
          operationId: "desktop-op-v1-safe",
          operation: {
            kind: "run.steer",
            input: {
              idempotencyKey: "renderer-controlled",
              runId: "run-safe",
              sessionId: "session-safe",
              text: "continue",
            },
          },
        },
      ]),
    ).toThrow(DesktopHostValidationError);
    expect(() =>
      parsePendingDesktopHostOperations([
        {
          operationId: "desktop-op-v1-safe",
          operation: {
            kind: "run.steer",
            input: { runId: "run-safe", sessionId: "session-safe", text: "continue" },
            requestId: "host-private-request",
          },
        },
      ]),
    ).toThrow(DesktopHostValidationError);
  });

  it("validates operation result acknowledgement deliveries", () => {
    expect(
      parseDesktopHostOperationDelivery("run.steer", {
        acknowledgementToken: "desktop-operation-ack-v1-safe",
        result: { accepted: true, receipt: { receiptId: "receipt-safe" } },
      }),
    ).toEqual({
      acknowledgementToken: "desktop-operation-ack-v1-safe",
      result: { accepted: true, receipt: { receiptId: "receipt-safe" } },
    });

    expect(() =>
      parseDesktopHostOperationDelivery("run.steer", {
        acknowledgementToken: "host-private-token",
        result: { accepted: true, receipt: { receiptId: "receipt-safe" } },
      }),
    ).toThrow(DesktopHostValidationError);
  });

  it("accepts acknowledgement tokens while rejecting raw host cursor authority", () => {
    expect(
      parseDesktopHostEventDelivery({
        acknowledgementToken: "desktop-event-ack-v1-safe",
        event: { delivery: { record: { eventId: "event-safe" } } },
      }),
    ).toEqual({
      acknowledgementToken: "desktop-event-ack-v1-safe",
      event: { delivery: { record: { eventId: "event-safe" } } },
    });

    expect(() =>
      parseDesktopHostEventDelivery({
        acknowledgementToken: "desktop-event-ack-v1-safe",
        event: {
          delivery: {
            cursor: "host-private-cursor",
            record: { eventId: "event-safe" },
          },
        },
      }),
    ).toThrow(DesktopHostValidationError);
  });
});
