import type { DesktopWorkspaceIntent } from "../bridge/desktop";
import type { DesktopHostAcknowledgementError, DesktopHostClient } from "../generated/host/client";
import type {
  ApprovalListResult,
  ApprovalShowResult,
  CatalogListResult,
  ClarificationListResult,
  ClarificationResolveIntent,
  ConfigGetResult,
  ConfigValidateResult,
  DeferredListResult,
  DeferredShowResult,
  DesktopHostInvocation,
  DesktopHostResultMap,
  ProfileGetResult,
  SessionGetResult,
  SessionListResult,
  WorkspaceListResult,
} from "../generated/host/types";

export type WorkspaceSummary = WorkspaceListResult["workspaces"][number];
export type SessionSummary = SessionListResult["sessions"][number];
export type RunSummary = SessionGetResult["runs"][number];
export type ApprovalSummary = ApprovalListResult["approvals"][number];
export type ClarificationSummary = ClarificationListResult["clarifications"][number];
export type DeferredSummary = DeferredListResult["deferred"][number];
export type ApprovalDetail = ApprovalShowResult["approval"];
export type DeferredDetail = DeferredShowResult["deferred"];
export type ClarificationAnswer = ClarificationResolveIntent["answers"][number];
export type ProfileSummary = CatalogListResult["profiles"][number];
export type ModelSelection = CatalogListResult["selection"];
export type ProfileDetail = ProfileGetResult["profile"];
export type RuntimeConfigDocument = ConfigGetResult["config"];
export type RuntimeConfigStatus = ConfigGetResult["status"];
export type RuntimeConfigValidation = ConfigValidateResult["validation"];

export type ProductPhase = "loading" | "ready" | "error";
export type SessionLoadState =
  | { readonly kind: "idle" }
  | { readonly kind: "loading"; readonly sessionId: string }
  | { readonly kind: "ready"; readonly sessionId: string }
  | { readonly kind: "error"; readonly sessionId: string };
export type HostSubscription = Awaited<ReturnType<DesktopHostClient["subscribe"]>>;
export type TranscriptLifecycle = "streaming" | "finished";

export type TranscriptMessage = {
  readonly messageId: string;
  readonly text: string;
  readonly lifecycle: TranscriptLifecycle;
  readonly lastSequence: string;
};

export type RunTranscript = {
  readonly lastSequence: string;
  readonly messageOrder: readonly string[];
  readonly messages: Readonly<Record<string, TranscriptMessage>>;
};

export type RunTranscripts = Readonly<Record<string, RunTranscript>>;

export type OwnedSubscription = {
  readonly generation: number;
  readonly ownership: number;
  readonly value: HostSubscription;
};

export type RecoverableOperationKind =
  | "workspace.register"
  | "session.create"
  | "run.start"
  | "run.resume"
  | "run.steer"
  | "run.interrupt"
  | "approval.decide"
  | "clarification.resolve"
  | "deferred.complete"
  | "deferred.fail"
  | "model.select"
  | "config.update"
  | "config.reload"
  | "config.activate"
  | "config.discard";

export type RecoverableInvocation = DesktopHostInvocation<RecoverableOperationKind>;
export type RecoverableResult = DesktopHostResultMap[RecoverableOperationKind];

export type PendingProductOperation =
  | {
      readonly recovery: "execute";
      readonly invocation: RecoverableInvocation;
      readonly workspaceIntent?: DesktopWorkspaceIntent;
    }
  | {
      readonly recovery: "acknowledge";
      readonly invocation: RecoverableInvocation;
      readonly failure: DesktopHostAcknowledgementError<RecoverableOperationKind>;
      readonly workspaceIntent?: DesktopWorkspaceIntent;
    };
