import type { DesktopHostClient } from "../generated/host/client";
import type { DesktopPageToken } from "../generated/host/types";
import type {
  ApprovalSummary,
  ClarificationSummary,
  DeferredSummary,
  RunSummary,
  WorkspaceSummary,
} from "./workspaceProductTypes";

const MAX_CATALOG_PAGES = 20;

export async function listAllWorkspaces(
  host: DesktopHostClient,
): Promise<readonly WorkspaceSummary[]> {
  const workspaces: WorkspaceSummary[] = [];
  let pageToken: DesktopPageToken | undefined;
  for (let page = 0; page < MAX_CATALOG_PAGES; page += 1) {
    const result = await host.workspaceList(pageToken === undefined ? {} : { pageToken });
    workspaces.push(...result.workspaces);
    if (!result.page.hasMore || result.page.nextPageToken === undefined) break;
    pageToken = result.page.nextPageToken;
  }
  return workspaces;
}

export async function listAllApprovals(
  host: DesktopHostClient,
  sessionId?: string,
): Promise<readonly ApprovalSummary[]> {
  const approvals: ApprovalSummary[] = [];
  let pageToken: DesktopPageToken | undefined;
  for (let page = 0; page < MAX_CATALOG_PAGES; page += 1) {
    const result = await host.approvalList({
      state: "unresolved",
      ...(sessionId === undefined ? {} : { sessionId }),
      ...(pageToken === undefined ? {} : { pageToken }),
    });
    approvals.push(...result.approvals);
    if (!result.page.hasMore || result.page.nextPageToken === undefined) return approvals;
    pageToken = result.page.nextPageToken;
  }
  throw new Error("approval discovery exceeds the bounded Desktop page budget");
}

export async function listAllClarifications(
  host: DesktopHostClient,
  sessionId?: string,
): Promise<readonly ClarificationSummary[]> {
  const clarifications: ClarificationSummary[] = [];
  let pageToken: DesktopPageToken | undefined;
  for (let page = 0; page < MAX_CATALOG_PAGES; page += 1) {
    const result = await host.clarificationList({
      state: "unresolved",
      ...(sessionId === undefined ? {} : { sessionId }),
      ...(pageToken === undefined ? {} : { pageToken }),
    });
    clarifications.push(...result.clarifications);
    if (!result.page.hasMore || result.page.nextPageToken === undefined) return clarifications;
    pageToken = result.page.nextPageToken;
  }
  throw new Error("clarification discovery exceeds the bounded Desktop page budget");
}

export async function listAllDeferred(
  host: DesktopHostClient,
  sessionId?: string,
): Promise<readonly DeferredSummary[]> {
  const deferred: DeferredSummary[] = [];
  let pageToken: DesktopPageToken | undefined;
  for (let page = 0; page < MAX_CATALOG_PAGES; page += 1) {
    const result = await host.deferredList({
      state: "unresolved",
      ...(sessionId === undefined ? {} : { sessionId }),
      ...(pageToken === undefined ? {} : { pageToken }),
    });
    deferred.push(...result.deferred);
    if (!result.page.hasMore || result.page.nextPageToken === undefined) return deferred;
    pageToken = result.page.nextPageToken;
  }
  throw new Error("deferred discovery exceeds the bounded Desktop page budget");
}

export async function listResolvedInteractionsForWaitingRuns(
  host: DesktopHostClient,
  runs: readonly RunSummary[],
): Promise<{
  readonly approvals: readonly ApprovalSummary[];
  readonly clarifications: readonly ClarificationSummary[];
  readonly deferred: readonly DeferredSummary[];
}> {
  const waiting = runs.filter((run) => run.status === "waiting");
  const pages = await Promise.all(
    waiting.map(async (run) => {
      const scope = { runId: run.runId, sessionId: run.sessionId, state: "resolved" as const };
      const [approvals, clarifications, deferred] = await Promise.all([
        host.approvalList(scope),
        host.clarificationList(scope),
        host.deferredList(scope),
      ]);
      return {
        approvals: approvals.approvals,
        clarifications: clarifications.clarifications,
        deferred: deferred.deferred,
      };
    }),
  );
  return {
    approvals: pages
      .flatMap((page) => page.approvals)
      .filter((approval) => approval.status !== "pending"),
    clarifications: pages
      .flatMap((page) => page.clarifications)
      .filter((clarification) => clarification.status !== "pending"),
    deferred: pages
      .flatMap((page) => page.deferred)
      .filter(
        (record) =>
          record.status !== "pending" && record.status !== "running" && record.status !== "waiting",
      ),
  };
}
