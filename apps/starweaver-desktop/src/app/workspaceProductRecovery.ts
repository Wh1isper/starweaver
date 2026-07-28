import {
  type DesktopWorkspaceIntent,
  executeDesktopWorkspaceRegistration,
} from "../bridge/desktop";
import {
  DesktopHostAcknowledgementError,
  type DesktopHostClient,
  DesktopHostExecutionError,
} from "../generated/host/client";
import type { DesktopHostInvocation, DesktopHostOperationKind } from "../generated/host/types";
import type {
  PendingProductOperation,
  RecoverableInvocation,
  RecoverableOperationKind,
  RecoverableResult,
} from "./workspaceProductTypes";

const RECOVERABLE_OPERATION_KINDS = new Set<DesktopHostOperationKind>([
  "workspace.register",
  "session.create",
  "run.start",
  "run.resume",
  "run.steer",
  "run.interrupt",
  "approval.decide",
  "clarification.resolve",
  "deferred.complete",
  "deferred.fail",
  "model.select",
  "config.update",
  "config.reload",
  "config.activate",
  "config.discard",
]);

export function isRecoverableInvocation(
  invocation: DesktopHostInvocation,
): invocation is RecoverableInvocation {
  return RECOVERABLE_OPERATION_KINDS.has(invocation.operation.kind);
}

export function pendingProductOperationFromFailure(
  error: unknown,
  workspaceIntent?: DesktopWorkspaceIntent,
): PendingProductOperation | undefined {
  if (error instanceof DesktopHostExecutionError && isRecoverableInvocation(error.invocation)) {
    return {
      recovery: "execute",
      invocation: error.invocation,
      workspaceIntent,
    };
  }
  if (
    error instanceof DesktopHostAcknowledgementError &&
    isRecoverableInvocation(error.invocation)
  ) {
    return {
      recovery: "acknowledge",
      invocation: error.invocation,
      failure: error as DesktopHostAcknowledgementError<RecoverableOperationKind>,
      workspaceIntent,
    };
  }
  return undefined;
}

export async function executePendingProductOperation(
  host: DesktopHostClient,
  pending: PendingProductOperation,
): Promise<RecoverableResult> {
  const { invocation } = pending;
  if (pending.recovery === "acknowledge") {
    return host.retryAcknowledgement(pending.failure);
  }
  if (invocation.operation.kind === "workspace.register") {
    return executeDesktopWorkspaceRegistration(
      invocation as DesktopHostInvocation<"workspace.register">,
      pending.workspaceIntent,
    );
  }
  return host.execute(invocation);
}
