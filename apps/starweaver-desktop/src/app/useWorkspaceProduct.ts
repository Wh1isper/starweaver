import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  type DesktopWorkspaceIntent,
  executeDesktopWorkspaceRegistration,
} from "../bridge/desktop";
import {
  DesktopHostAcknowledgementError,
  DesktopHostClient,
  DesktopHostExecutionError,
} from "../generated/host/client";
import type {
  DesktopHostInvocation,
  DesktopHostResultMap,
  DesktopPageToken,
  RunListResult,
  RunResumeResult,
  RunStartResult,
  SessionGetResult,
} from "../generated/host/types";
import {
  ACTIVE_RUN_STATES,
  applyRunHostEvent,
  applyTranscriptHostEvent,
  replaceById,
  replaceRun,
  transcriptText,
} from "./workspaceProductEvents";
import {
  listAllApprovals,
  listAllClarifications,
  listAllDeferred,
  listAllWorkspaces,
  listResolvedInteractionsForWaitingRuns,
} from "./workspaceProductQueries";
import {
  executePendingProductOperation,
  isRecoverableInvocation,
  pendingProductOperationFromFailure,
} from "./workspaceProductRecovery";
import { deriveProfileReadiness } from "./workspaceProductSettings";
import type {
  ApprovalDetail,
  ApprovalSummary,
  ClarificationAnswer,
  ClarificationSummary,
  DeferredDetail,
  DeferredSummary,
  ModelSelection,
  OwnedSubscription,
  PendingProductOperation,
  ProductPhase,
  ProfileDetail,
  ProfileSummary,
  RecoverableInvocation,
  RecoverableOperationKind,
  RecoverableResult,
  RunSummary,
  RunTranscripts,
  RuntimeConfigDocument,
  RuntimeConfigStatus,
  RuntimeConfigValidation,
  SessionLoadState,
  SessionSummary,
  WorkspaceSummary,
} from "./workspaceProductTypes";

export type {
  ApprovalDetail,
  ApprovalSummary,
  ClarificationAnswer,
  ClarificationSummary,
  DeferredDetail,
  DeferredSummary,
  ModelSelection,
  ProfileDetail,
  ProfileSummary,
  RunSummary,
  RuntimeConfigDocument,
  RuntimeConfigStatus,
  RuntimeConfigValidation,
  SessionSummary,
  WorkspaceSummary,
} from "./workspaceProductTypes";

const TRANSCRIPT_HYDRATION_CONCURRENCY = 4;

export function useWorkspaceProduct(options: { readonly conversationSessionId?: string } = {}) {
  const { conversationSessionId } = options;
  const host = useMemo(() => new DesktopHostClient(), []);
  const [phase, setPhase] = useState<ProductPhase>("loading");
  const [workspaces, setWorkspaces] = useState<readonly WorkspaceSummary[]>([]);
  const [sessions, setSessions] = useState<readonly SessionSummary[]>([]);
  const [sessionPageToken, setSessionPageToken] = useState<DesktopPageToken>();
  const [sessionsLoadingMore, setSessionsLoadingMore] = useState(false);
  const [selectedSession, setSelectedSession] = useState<SessionSummary | undefined>();
  const [sessionLoadState, setSessionLoadState] = useState<SessionLoadState>({ kind: "idle" });
  const [runs, setRuns] = useState<readonly RunSummary[]>([]);
  const [runPageToken, setRunPageToken] = useState<DesktopPageToken>();
  const [runsLoadingMore, setRunsLoadingMore] = useState(false);
  const [checkedRunControlIds, setCheckedRunControlIds] = useState<ReadonlySet<string>>(new Set());
  const [controllableRunIds, setControllableRunIds] = useState<ReadonlySet<string>>(new Set());
  const [transcripts, setTranscripts] = useState<RunTranscripts>({});
  const [approvals, setApprovals] = useState<readonly ApprovalSummary[]>([]);
  const [clarifications, setClarifications] = useState<readonly ClarificationSummary[]>([]);
  const [deferred, setDeferred] = useState<readonly DeferredSummary[]>([]);
  const [approvalDetails, setApprovalDetails] = useState<Readonly<Record<string, ApprovalDetail>>>(
    {},
  );
  const [approvalDetailErrors, setApprovalDetailErrors] = useState<ReadonlySet<string>>(new Set());
  const [deferredDetails, setDeferredDetails] = useState<Readonly<Record<string, DeferredDetail>>>(
    {},
  );
  const [deferredDetailErrors, setDeferredDetailErrors] = useState<ReadonlySet<string>>(new Set());
  const [interactionsLoading, setInteractionsLoading] = useState(false);
  const [profiles, setProfiles] = useState<readonly ProfileSummary[]>([]);
  const [modelSelection, setModelSelection] = useState<ModelSelection>();
  const [profileDetail, setProfileDetail] = useState<ProfileDetail>();
  const [runtimeConfig, setRuntimeConfig] = useState<RuntimeConfigDocument>();
  const [runtimeConfigStatus, setRuntimeConfigStatus] = useState<RuntimeConfigStatus>();
  const [runtimeConfigValidation, setRuntimeConfigValidation] = useState<RuntimeConfigValidation>();
  const [reloadCandidateEtag, setReloadCandidateEtag] = useState<string>();
  const [settingsLoading, setSettingsLoading] = useState(false);
  const [activeOperation, setActiveOperation] = useState<RecoverableOperationKind | "recovery">();
  const busy = activeOperation !== undefined;
  const [notice, setNotice] = useState<string | undefined>();
  const [pendingOperationIds, setPendingOperationIds] = useState<readonly string[]>([]);
  const selectionGeneration = useRef(0);
  const interactionRefreshGeneration = useRef(0);
  const interactionLiveGeneration = useRef(0);
  const settingsRefreshGeneration = useRef(0);
  const runStateEpoch = useRef(0);
  const runsRef = useRef<readonly RunSummary[]>([]);
  const subscriptionOwnership = useRef(0);
  const subscriptionOwners = useRef(new Map<string, number>());
  const subscriptions = useRef(new Map<string, OwnedSubscription>());
  const pendingOperations = useRef(new Map<string, PendingProductOperation>());
  const sessionPageLoading = useRef(false);
  const rendererVisible = useRef(
    typeof document === "undefined" || document.visibilityState !== "hidden",
  );

  const updateRuns = useCallback(
    (
      update: readonly RunSummary[] | ((current: readonly RunSummary[]) => readonly RunSummary[]),
    ) => {
      const next = typeof update === "function" ? update(runsRef.current) : update;
      runStateEpoch.current += 1;
      runsRef.current = next;
      setRuns(next);
      return runStateEpoch.current;
    },
    [],
  );

  const closeSubscriptions = useCallback(async () => {
    subscriptionOwnership.current += 1;
    subscriptionOwners.current.clear();
    const current = [...subscriptions.current.values()];
    subscriptions.current.clear();
    await Promise.all(current.map(({ value }) => value.close().catch(() => undefined)));
  }, []);

  const setPendingOperation = useCallback(
    (operationId: string, pending: PendingProductOperation | undefined) => {
      if (pending === undefined) pendingOperations.current.delete(operationId);
      else pendingOperations.current.set(operationId, pending);
      setPendingOperationIds([...pendingOperations.current.keys()]);
    },
    [],
  );

  const rememberOperationFailure = useCallback(
    (
      invocation: RecoverableInvocation,
      error: unknown,
      workspaceIntent?: DesktopWorkspaceIntent,
    ): boolean => {
      const pending = pendingProductOperationFromFailure(error, workspaceIntent);
      setPendingOperation(invocation.operationId, pending);
      return pending !== undefined;
    },
    [setPendingOperation],
  );

  const executeRecoverable = useCallback(
    async <K extends RecoverableOperationKind>(
      invocation: DesktopHostInvocation<K>,
      execute: () => Promise<DesktopHostResultMap[K]> = () => host.execute(invocation),
      workspaceIntent?: DesktopWorkspaceIntent,
    ): Promise<DesktopHostResultMap[K]> => {
      try {
        const result = await execute();
        setPendingOperation(invocation.operationId, undefined);
        return result;
      } catch (error: unknown) {
        rememberOperationFailure(invocation, error, workspaceIntent);
        throw error;
      }
    },
    [host, rememberOperationFailure, setPendingOperation],
  );

  const recoverPendingOperation = useCallback(
    async (pending: PendingProductOperation): Promise<RecoverableResult> => {
      const { invocation } = pending;
      try {
        const result = await executePendingProductOperation(host, pending);
        setPendingOperation(invocation.operationId, undefined);
        return result;
      } catch (error: unknown) {
        rememberOperationFailure(invocation, error, pending.workspaceIntent);
        throw error;
      }
    },
    [host, rememberOperationFailure, setPendingOperation],
  );

  const refreshInteractions = useCallback(
    async (sessionRuns = runsRef.current) => {
      const generation = interactionRefreshGeneration.current + 1;
      interactionRefreshGeneration.current = generation;
      setInteractionsLoading(true);
      try {
        for (let attempt = 0; attempt < 3; attempt += 1) {
          const liveGeneration = interactionLiveGeneration.current;
          const [nextApprovals, nextClarifications, nextDeferred, resolved] = await Promise.all([
            listAllApprovals(host, conversationSessionId),
            listAllClarifications(host, conversationSessionId),
            listAllDeferred(host, conversationSessionId),
            listResolvedInteractionsForWaitingRuns(host, sessionRuns),
          ]);
          if (interactionRefreshGeneration.current !== generation) return;
          if (interactionLiveGeneration.current !== liveGeneration) continue;
          setApprovals([...nextApprovals, ...resolved.approvals]);
          setClarifications([...nextClarifications, ...resolved.clarifications]);
          setDeferred([...nextDeferred, ...resolved.deferred]);
          return;
        }
        throw new Error("interaction discovery did not reach a stable live generation");
      } catch {
        if (interactionRefreshGeneration.current !== generation) return;
        setNotice(
          "Interactions could not be refreshed. Durable requests remain available locally.",
        );
      } finally {
        if (interactionRefreshGeneration.current === generation) setInteractionsLoading(false);
      }
    },
    [conversationSessionId, host],
  );

  const refreshSettings = useCallback(async () => {
    const generation = settingsRefreshGeneration.current + 1;
    if (conversationSessionId !== undefined) {
      settingsRefreshGeneration.current = generation;
      setSettingsLoading(false);
      return;
    }
    settingsRefreshGeneration.current = generation;
    setSettingsLoading(true);
    try {
      const [catalog, config] = await Promise.all([host.catalogList({}), host.configGet({})]);
      if (settingsRefreshGeneration.current !== generation) return;
      setProfiles(catalog.profiles);
      setModelSelection(catalog.selection);
      setRuntimeConfig(config.config);
      setRuntimeConfigStatus(config.status);
      const selected = catalog.profiles.find(
        (profile) => profile.name === catalog.selection.selectedProfile,
      );
      if (selected === undefined) {
        setProfileDetail(undefined);
      } else {
        const detail = await host.profileGet({ name: selected.name });
        if (settingsRefreshGeneration.current !== generation) return;
        setProfileDetail(detail.profile);
      }
    } catch {
      if (settingsRefreshGeneration.current !== generation) return;
      setNotice(
        "Profile and runtime settings could not be refreshed. Existing runs remain unchanged.",
      );
    } finally {
      if (settingsRefreshGeneration.current === generation) setSettingsLoading(false);
    }
  }, [conversationSessionId, host]);

  const refreshRunControllability = useCallback(
    async (sessionRuns: readonly RunSummary[], generation: number, epoch: number) => {
      let candidateRuns = sessionRuns;
      let candidateEpoch = epoch;
      for (let attempt = 0; attempt < 3; attempt += 1) {
        const activeRuns = candidateRuns.filter((run) => ACTIVE_RUN_STATES.has(run.status));
        const statuses = await Promise.all(
          activeRuns.map((run) =>
            host.runStatus({ sessionId: run.sessionId, runId: run.runId }).catch(() => undefined),
          ),
        );
        if (selectionGeneration.current !== generation) return;
        if (runStateEpoch.current !== candidateEpoch) {
          candidateRuns = runsRef.current;
          candidateEpoch = runStateEpoch.current;
          continue;
        }
        setCheckedRunControlIds(
          new Set(
            statuses.flatMap((status, index) =>
              status === undefined
                ? []
                : [activeRuns[index]?.runId].filter((id): id is string => id !== undefined),
            ),
          ),
        );
        setControllableRunIds(
          new Set(
            statuses.flatMap((status, index) =>
              status?.controllableByCurrentHost && activeRuns[index] !== undefined
                ? [activeRuns[index].runId]
                : [],
            ),
          ),
        );
        return;
      }
    },
    [host],
  );

  const refreshSession = useCallback(
    async (sessionId: string, generation: number): Promise<SessionGetResult | undefined> => {
      const epoch = runStateEpoch.current + 1;
      runStateEpoch.current = epoch;
      const result = await host.sessionGet({ sessionId });
      if (selectionGeneration.current !== generation || runStateEpoch.current !== epoch) {
        return undefined;
      }
      let sessionRuns = result.runs;
      let nextRunPageToken: DesktopPageToken | undefined;
      if (result.runs.length === 100) {
        try {
          const page = await host.runList({ sessionId });
          if (selectionGeneration.current !== generation || runStateEpoch.current !== epoch) {
            return undefined;
          }
          sessionRuns = [...page.runs].reverse();
          nextRunPageToken = page.page.hasMore ? page.page.nextPageToken : undefined;
        } catch {
          setNotice(
            "Earlier runs could not be checked. The newest durable history remains available.",
          );
        }
      }
      setSelectedSession(result.session);
      setRunPageToken(nextRunPageToken);
      runsRef.current = sessionRuns;
      setRuns(sessionRuns);
      setSessions((current) =>
        current.map((session) =>
          session.sessionId === result.session.sessionId ? result.session : session,
        ),
      );
      await Promise.all([
        refreshInteractions(sessionRuns),
        refreshRunControllability(sessionRuns, generation, epoch),
      ]);
      return { ...result, runs: sessionRuns };
    },
    [host, refreshInteractions, refreshRunControllability],
  );

  const attachRun = useCallback(
    async (sessionId: string, runId: string, generation: number, keepLive: boolean) => {
      if (selectionGeneration.current !== generation || !rendererVisible.current) return;
      const ownership = subscriptionOwnership.current + 1;
      subscriptionOwnership.current = ownership;
      subscriptionOwners.current.set(runId, ownership);
      const previous = subscriptions.current.get(runId);
      subscriptions.current.delete(runId);
      if (previous !== undefined) await previous.value.close().catch(() => undefined);
      if (selectionGeneration.current !== generation || !rendererVisible.current) {
        if (subscriptionOwners.current.get(runId) === ownership) {
          subscriptionOwners.current.delete(runId);
        }
        return;
      }
      try {
        const current = await host.subscribe({ sessionId, runId }, async (event) => {
          if (
            selectionGeneration.current !== generation ||
            !rendererVisible.current ||
            subscriptionOwners.current.get(runId) !== ownership
          ) {
            return;
          }
          const payload = event.delivery.record.event;
          if (payload.kind === "run_changed" || payload.kind === "output_available") {
            updateRuns((currentRuns) => applyRunHostEvent(currentRuns, event));
          }
          setTranscripts((existing) => applyTranscriptHostEvent(existing, event));
          if (payload.kind === "approval_changed") {
            interactionLiveGeneration.current += 1;
            setApprovals((existing) =>
              replaceById(existing, payload.approval, (approval) => approval.approvalId),
            );
          } else if (payload.kind === "clarification_changed") {
            interactionLiveGeneration.current += 1;
            setClarifications((existing) =>
              replaceById(
                existing,
                payload.clarification,
                (clarification) => clarification.clarificationId,
              ),
            );
          } else if (payload.kind === "deferred_changed") {
            interactionLiveGeneration.current += 1;
            setDeferred((existing) =>
              replaceById(existing, payload.deferred, (record) => record.deferredId),
            );
          } else if (payload.kind === "run_changed" && payload.run.status === "waiting") {
            void refreshInteractions(runsRef.current);
          }
        });
        if (
          selectionGeneration.current !== generation ||
          !rendererVisible.current ||
          subscriptionOwners.current.get(runId) !== ownership
        ) {
          await current.close().catch(() => undefined);
          return;
        }
        const owned = { generation, ownership, value: current };
        subscriptions.current.set(runId, owned);
        const releaseOwnership = () => {
          if (subscriptions.current.get(runId) === owned) subscriptions.current.delete(runId);
          if (subscriptionOwners.current.get(runId) === ownership) {
            subscriptionOwners.current.delete(runId);
          }
        };
        const completion = current.done.then(
          async () => {
            releaseOwnership();
            if (selectionGeneration.current !== generation || !rendererVisible.current || !keepLive)
              return;
            await refreshSession(sessionId, generation)
              .then((result) => {
                if (
                  result?.runs.some(
                    (run) => run.runId === runId && ACTIVE_RUN_STATES.has(run.status),
                  )
                ) {
                  setNotice(
                    "Live updates stopped before this run finished. Reopen the conversation to reconnect.",
                  );
                }
              })
              .catch(() => {
                setNotice(
                  "The latest run state could not be refreshed. Try opening this conversation again.",
                );
              });
          },
          () => {
            const stillOwned = subscriptionOwners.current.get(runId) === ownership;
            releaseOwnership();
            if (
              selectionGeneration.current === generation &&
              rendererVisible.current &&
              stillOwned
            ) {
              setNotice(
                "Live updates are temporarily unavailable. Durable run state is still preserved.",
              );
            }
          },
        );
        if (keepLive) {
          void completion;
        } else {
          await current.caughtUp;
          if (subscriptionOwners.current.get(runId) === ownership) {
            await current.close();
          }
          await completion;
        }
      } catch {
        if (subscriptions.current.get(runId)?.ownership === ownership) {
          subscriptions.current.delete(runId);
        }
        if (subscriptionOwners.current.get(runId) === ownership) {
          subscriptionOwners.current.delete(runId);
        }
        if (selectionGeneration.current === generation && rendererVisible.current) {
          setNotice(
            "Live updates are temporarily unavailable. Durable run state is still preserved.",
          );
        }
      }
    },
    [host, refreshInteractions, refreshSession, updateRuns],
  );

  const hydrateRunTranscripts = useCallback(
    async (sessionId: string, sessionRuns: readonly RunSummary[], generation: number) => {
      const active = sessionRuns.filter((run) => ACTIVE_RUN_STATES.has(run.status));
      await Promise.all(active.map((run) => attachRun(sessionId, run.runId, generation, true)));
      const terminal = sessionRuns.filter((run) => !ACTIVE_RUN_STATES.has(run.status));
      let nextIndex = 0;
      const hydrateNext = async () => {
        while (selectionGeneration.current === generation && rendererVisible.current) {
          const run = terminal[nextIndex];
          nextIndex += 1;
          if (run === undefined) return;
          await attachRun(sessionId, run.runId, generation, false);
        }
      };
      await Promise.all(
        Array.from(
          { length: Math.min(TRANSCRIPT_HYDRATION_CONCURRENCY, terminal.length) },
          hydrateNext,
        ),
      );
    },
    [attachRun],
  );

  const selectSession = useCallback(
    async (session: SessionSummary) => {
      const generation = selectionGeneration.current + 1;
      selectionGeneration.current = generation;
      setSelectedSession(session);
      setSessionLoadState({ kind: "loading", sessionId: session.sessionId });
      setRunPageToken(undefined);
      updateRuns([]);
      setCheckedRunControlIds(new Set());
      setControllableRunIds(new Set());
      setTranscripts({});
      setNotice(undefined);
      await closeSubscriptions();
      try {
        const result = await refreshSession(session.sessionId, generation);
        if (result !== undefined) {
          setSessionLoadState({ kind: "ready", sessionId: session.sessionId });
          void hydrateRunTranscripts(session.sessionId, result.runs, generation);
        }
      } catch {
        if (selectionGeneration.current === generation) {
          setSessionLoadState({ kind: "error", sessionId: session.sessionId });
          setNotice("This conversation could not be loaded from local history.");
        }
      }
    },
    [closeSubscriptions, hydrateRunTranscripts, refreshSession, updateRuns],
  );

  const refreshCatalog = useCallback(
    async (preferredSessionId?: string) => {
      setPhase("loading");
      try {
        if (conversationSessionId !== undefined) {
          const [nextWorkspaces, routed] = await Promise.all([
            listAllWorkspaces(host),
            host.sessionGet({ sessionId: conversationSessionId }),
          ]);
          setWorkspaces(nextWorkspaces.filter((workspace) => workspace.state === "active"));
          setSessions([routed.session]);
          setSessionPageToken(undefined);
          setPhase("ready");
          await selectSession(routed.session);
          return;
        }
        const [nextWorkspaces, page] = await Promise.all([
          listAllWorkspaces(host),
          host.sessionList({}),
        ]);
        setWorkspaces(nextWorkspaces.filter((workspace) => workspace.state === "active"));
        const visibleSessions = page.sessions.filter((session) => session.status !== "deleted");
        setSessions(visibleSessions);
        setSessionPageToken(page.page.hasMore ? page.page.nextPageToken : undefined);
        setPhase("ready");
        let preferred = visibleSessions.find((session) => session.sessionId === preferredSessionId);
        if (preferred === undefined && preferredSessionId !== undefined) {
          const routed = await host.sessionGet({ sessionId: preferredSessionId });
          preferred = routed.session;
          setSessions((current) => [routed.session, ...current]);
        }
        preferred ??= visibleSessions[0];
        if (preferred !== undefined) await selectSession(preferred);
        else setSessionLoadState({ kind: "idle" });
      } catch {
        setPhase("error");
      }
    },
    [conversationSessionId, host, selectSession],
  );

  useEffect(() => {
    if (typeof document === "undefined") return;

    const onVisibilityChange = () => {
      rendererVisible.current = document.visibilityState !== "hidden";
      if (!rendererVisible.current) {
        void closeSubscriptions();
        return;
      }
      const sessionId = selectedSession?.sessionId;
      if (sessionId === undefined) return;
      const generation = selectionGeneration.current;
      void refreshSession(sessionId, generation)
        .then((result) => {
          if (result !== undefined && rendererVisible.current) {
            void hydrateRunTranscripts(sessionId, result.runs, generation);
          }
        })
        .catch(() => {
          if (selectionGeneration.current === generation && rendererVisible.current) {
            setNotice(
              "This conversation could not reconnect to live updates. Its durable history remains available.",
            );
          }
        });
    };

    document.addEventListener("visibilitychange", onVisibilityChange);
    return () => document.removeEventListener("visibilitychange", onVisibilityChange);
  }, [closeSubscriptions, hydrateRunTranscripts, refreshSession, selectedSession?.sessionId]);

  const loadMoreSessions = useCallback(async () => {
    if (
      conversationSessionId !== undefined ||
      sessionPageToken === undefined ||
      sessionPageLoading.current
    ) {
      return;
    }
    sessionPageLoading.current = true;
    setSessionsLoadingMore(true);
    try {
      const page = await host.sessionList({ pageToken: sessionPageToken });
      const visible = page.sessions.filter((session) => session.status !== "deleted");
      setSessions((current) => {
        const known = new Set(current.map((session) => session.sessionId));
        return [...current, ...visible.filter((session) => !known.has(session.sessionId))];
      });
      setSessionPageToken(page.page.hasMore ? page.page.nextPageToken : undefined);
    } catch {
      setNotice("Older conversations could not be loaded. The current history remains available.");
    } finally {
      sessionPageLoading.current = false;
      setSessionsLoadingMore(false);
    }
  }, [conversationSessionId, host, sessionPageToken]);

  const loadMoreRuns = useCallback(async () => {
    const sessionId = selectedSession?.sessionId;
    const pageToken = runPageToken;
    if (sessionId === undefined || pageToken === undefined || runsLoadingMore) return;
    const generation = selectionGeneration.current;
    setRunsLoadingMore(true);
    try {
      const page = (await host.runList({ sessionId, pageToken })) as RunListResult;
      if (selectionGeneration.current !== generation) return;
      const older = [...page.runs].reverse();
      updateRuns((current) => {
        const known = new Set(current.map((run) => run.runId));
        return [...older.filter((run) => !known.has(run.runId)), ...current];
      });
      setRunPageToken(page.page.hasMore ? page.page.nextPageToken : undefined);
      void hydrateRunTranscripts(sessionId, older, generation);
    } catch {
      if (selectionGeneration.current === generation) {
        setNotice("Earlier runs could not be loaded. The current conversation remains available.");
      }
    } finally {
      if (selectionGeneration.current === generation) setRunsLoadingMore(false);
    }
  }, [
    host,
    hydrateRunTranscripts,
    runPageToken,
    runsLoadingMore,
    selectedSession?.sessionId,
    updateRuns,
  ]);

  const reconcileDurableOperations = useCallback(async (): Promise<number> => {
    const durable = await host.pendingOperations();
    for (const invocation of durable) {
      if (!isRecoverableInvocation(invocation)) continue;
      setPendingOperation(invocation.operationId, {
        recovery: "execute",
        invocation,
      });
    }
    for (const pending of [...pendingOperations.current.values()]) {
      try {
        await recoverPendingOperation(pending);
      } catch {
        break;
      }
    }
    return pendingOperations.current.size;
  }, [host, recoverPendingOperation, setPendingOperation]);

  const retryPendingProductOperations = useCallback(async () => {
    setActiveOperation("recovery");
    setNotice(undefined);
    const preferredSessionId = selectedSession?.sessionId;
    try {
      for (const pending of [...pendingOperations.current.values()]) {
        try {
          await recoverPendingOperation(pending);
        } catch {
          break;
        }
      }
      await Promise.all([
        refreshCatalog(preferredSessionId),
        refreshInteractions(),
        refreshSettings(),
      ]);
      if (pendingOperations.current.size > 0) {
        setNotice(
          "Some local changes still have an unresolved outcome. Retry recovery without repeating the action.",
        );
      }
    } catch {
      setNotice("Pending local changes could not be reconciled with the runtime.");
    } finally {
      setActiveOperation(undefined);
    }
  }, [
    recoverPendingOperation,
    refreshCatalog,
    refreshInteractions,
    refreshSettings,
    selectedSession,
  ]);

  useEffect(() => {
    void (async () => {
      let pendingCount = 0;
      try {
        pendingCount = await reconcileDurableOperations();
      } catch {
        pendingCount = pendingOperations.current.size;
      }
      await Promise.all([
        refreshCatalog(conversationSessionId),
        refreshInteractions(),
        refreshSettings(),
      ]);
      if (pendingCount > 0) {
        setNotice(
          "A previous local change has an unresolved outcome. Retry recovery without repeating it.",
        );
      }
    })();
    return () => {
      selectionGeneration.current += 1;
      settingsRefreshGeneration.current += 1;
      void closeSubscriptions();
    };
  }, [
    closeSubscriptions,
    conversationSessionId,
    reconcileDurableOperations,
    refreshCatalog,
    refreshInteractions,
    refreshSettings,
  ]);

  const selectProfile = useCallback(
    async (profileName: string): Promise<boolean> => {
      if (!profiles.some((profile) => profile.name === profileName)) return false;
      if (
        [...pendingOperations.current.values()].some(
          (pending) => pending.invocation.operation.kind === "model.select",
        )
      ) {
        setNotice(
          "Recover the unresolved profile change before selecting another default profile.",
        );
        return false;
      }
      setActiveOperation("model.select");
      setNotice(undefined);
      try {
        const invocation = host.prepare({
          kind: "model.select",
          input: { profile: profileName },
        });
        const result = await executeRecoverable(invocation);
        setModelSelection(result.selection);
        try {
          const detail = await host.profileGet({ name: result.selection.selectedProfile });
          setProfileDetail(detail.profile);
          setNotice("Default profile updated for new runs.");
        } catch {
          setProfileDetail(undefined);
          setNotice(
            "Default profile updated for new runs, but its details could not be refreshed yet.",
          );
          void refreshSettings();
        }
        return true;
      } catch {
        setNotice(
          pendingOperations.current.size > 0
            ? "The profile change has an unresolved outcome. Retry recovery without selecting it again."
            : "This profile is no longer available in the active runtime catalog.",
        );
        return false;
      } finally {
        setActiveOperation(undefined);
      }
    },
    [executeRecoverable, host, profiles, refreshSettings],
  );

  const validateRuntimeConfig = useCallback(
    async (candidate: RuntimeConfigDocument): Promise<RuntimeConfigValidation | undefined> => {
      setSettingsLoading(true);
      try {
        const result = await host.configValidate({ candidate });
        setRuntimeConfigValidation(result.validation);
        return result.validation;
      } catch {
        setNotice("The runtime could not validate these settings.");
        return undefined;
      } finally {
        setSettingsLoading(false);
      }
    },
    [host],
  );

  const saveRuntimeConfig = useCallback(
    async (candidate: RuntimeConfigDocument): Promise<boolean> => {
      const activeEtag = runtimeConfigStatus?.active.etag;
      if (activeEtag === undefined || settingsLoading) return false;
      setActiveOperation("config.update");
      setNotice(undefined);
      try {
        const checked = await host.configValidate({ candidate });
        setRuntimeConfigValidation(checked.validation);
        if (!checked.validation.valid) {
          setNotice("Fix the validation errors before saving runtime settings.");
          return false;
        }
        const invocation = host.prepare({
          kind: "config.update",
          input: { candidate, expectedActiveEtag: activeEtag },
        });
        const result = await executeRecoverable(invocation);
        setRuntimeConfigStatus(result.status);
        setRuntimeConfigValidation(result.validation);
        await refreshSettings();
        setNotice(
          result.status.restartRequired
            ? "Runtime settings were staged and require a managed runtime restart."
            : "Runtime settings updated for new runs.",
        );
        return true;
      } catch {
        setNotice(
          pendingOperations.current.size > 0
            ? "The settings update has an unresolved outcome. Retry recovery without saving again."
            : "Runtime settings changed before this update could be applied.",
        );
        return false;
      } finally {
        setActiveOperation(undefined);
      }
    },
    [executeRecoverable, host, refreshSettings, runtimeConfigStatus, settingsLoading],
  );

  const previewRuntimeReload = useCallback(async (): Promise<boolean> => {
    const activeEtag = runtimeConfigStatus?.active.etag;
    if (activeEtag === undefined) return false;
    setSettingsLoading(true);
    setNotice(undefined);
    try {
      const invocation = host.prepare({
        kind: "config.reload",
        input: { expectedActiveEtag: activeEtag, mode: "dry_run" },
      });
      const result = await executeRecoverable(invocation);
      setReloadCandidateEtag(result.candidateEtag);
      setRuntimeConfigStatus(result.status);
      setRuntimeConfigValidation(result.validation);
      return true;
    } catch {
      setNotice("The authoritative runtime source could not be checked safely.");
      return false;
    } finally {
      setSettingsLoading(false);
    }
  }, [executeRecoverable, host, runtimeConfigStatus]);

  const commitRuntimeReload = useCallback(async (): Promise<boolean> => {
    const activeEtag = runtimeConfigStatus?.active.etag;
    if (activeEtag === undefined || reloadCandidateEtag === undefined) return false;
    setActiveOperation("config.reload");
    setNotice(undefined);
    try {
      const invocation = host.prepare({
        kind: "config.reload",
        input: {
          candidateEtag: reloadCandidateEtag,
          expectedActiveEtag: activeEtag,
          mode: "commit",
        },
      });
      const result = await executeRecoverable(invocation);
      setReloadCandidateEtag(undefined);
      setRuntimeConfigStatus(result.status);
      setRuntimeConfigValidation(result.validation);
      await refreshSettings();
      setNotice("The authoritative runtime settings were reloaded for new runs.");
      return true;
    } catch {
      setNotice(
        pendingOperations.current.size > 0
          ? "The reload outcome is unresolved. Retry recovery without committing again."
          : "The runtime source changed after preview; review it again before reloading.",
      );
      return false;
    } finally {
      setActiveOperation(undefined);
    }
  }, [executeRecoverable, host, refreshSettings, reloadCandidateEtag, runtimeConfigStatus]);

  const discardStagedRuntimeConfig = useCallback(async (): Promise<boolean> => {
    if (runtimeConfigStatus === undefined || !runtimeConfigStatus.restartRequired) return false;
    const desiredEtag = runtimeConfigStatus.desired.etag;
    setActiveOperation("config.discard");
    setNotice(undefined);
    try {
      const invocation = host.prepare({
        kind: "config.discard",
        input: { desiredEtag },
      });
      const result = await executeRecoverable(invocation);
      setRuntimeConfigStatus(result.status);
      await refreshSettings();
      setNotice("The staged runtime settings were discarded.");
      return true;
    } catch {
      setNotice(
        pendingOperations.current.size > 0
          ? "Discard has an unresolved outcome. Retry recovery without discarding again."
          : "The staged runtime settings changed before they could be discarded.",
      );
      return false;
    } finally {
      setActiveOperation(undefined);
    }
  }, [executeRecoverable, host, refreshSettings, runtimeConfigStatus]);

  const createWorkspace = useCallback(
    async (intent: DesktopWorkspaceIntent, displayLabel?: string) => {
      setActiveOperation("workspace.register");
      setNotice(undefined);
      try {
        const registration = host.prepare({
          kind: "workspace.register",
          input: displayLabel === undefined ? {} : { displayLabel },
        });
        const workspace = (
          await executeRecoverable(
            registration,
            () => executeDesktopWorkspaceRegistration(registration, intent),
            intent,
          )
        ).workspace;
        const creation = host.prepare({
          kind: "session.create",
          input: {
            workspaceId: workspace.workspaceId,
            title: "New conversation",
          },
        });
        const created = await executeRecoverable(creation);
        await refreshCatalog(created.session.sessionId);
      } catch {
        setNotice(
          pendingOperations.current.size > 0
            ? "Workspace setup has an unresolved outcome. Retry recovery without creating it again."
            : "Workspace setup was cancelled or could not be completed.",
        );
      } finally {
        setActiveOperation(undefined);
      }
    },
    [executeRecoverable, host, refreshCatalog],
  );

  const createSession = useCallback(
    async (workspace: WorkspaceSummary) => {
      setActiveOperation("session.create");
      setNotice(undefined);
      try {
        const creation = host.prepare({
          kind: "session.create",
          input: {
            workspaceId: workspace.workspaceId,
            title: "New conversation",
          },
        });
        const created = await executeRecoverable(creation);
        await refreshCatalog(created.session.sessionId);
      } catch {
        setNotice(
          pendingOperations.current.size > 0
            ? "Conversation creation has an unresolved outcome. Retry recovery without creating another one."
            : "A new conversation could not be created in this workspace.",
        );
      } finally {
        setActiveOperation(undefined);
      }
    },
    [executeRecoverable, host, refreshCatalog],
  );

  const sendPrompt = useCallback(
    async (text: string): Promise<boolean> => {
      if (
        selectedSession === undefined ||
        sessionLoadState.kind !== "ready" ||
        sessionLoadState.sessionId !== selectedSession.sessionId
      ) {
        return false;
      }
      const prompt = text.trim();
      if (prompt.length === 0) return false;
      const sessionId = selectedSession.sessionId;
      const generation = selectionGeneration.current;
      const pending = [...pendingOperations.current.values()].find(
        (candidate) =>
          candidate.invocation.operation.kind === "run.start" &&
          candidate.invocation.operation.input.sessionId === sessionId,
      );
      if (pending === undefined && Array.from(prompt).length > 65_536) {
        setNotice("A prompt can contain at most 65,536 Unicode characters.");
        return false;
      }
      const invocation =
        pending?.invocation.operation.kind === "run.start"
          ? (pending.invocation as DesktopHostInvocation<"run.start">)
          : host.prepare({
              kind: "run.start",
              input: {
                continuationMode: "preserve",
                input: [{ kind: "text", text: prompt }],
                sessionId,
              },
            });
      setActiveOperation("run.start");
      setNotice(undefined);
      try {
        const started = (
          pending === undefined
            ? await executeRecoverable(invocation)
            : await recoverPendingOperation(pending)
        ) as RunStartResult;
        if (selectionGeneration.current !== generation) return false;
        const epoch = updateRuns((current) => replaceRun(current, started.run));
        void refreshRunControllability(runsRef.current, generation, epoch);
        void attachRun(sessionId, started.run.runId, generation, true);
        return true;
      } catch (error: unknown) {
        if (selectionGeneration.current === generation) {
          if (error instanceof DesktopHostExecutionError) {
            setNotice(
              "The runtime outcome is unresolved. Retry to reconcile the same prompt without creating a duplicate run.",
            );
          } else if (error instanceof DesktopHostAcknowledgementError) {
            setNotice(
              "The prompt outcome is known, but local acknowledgement is pending. Retry to finish recovery without executing it again.",
            );
          } else {
            setNotice("The runtime rejected this prompt before a run could be started.");
          }
        }
        return false;
      } finally {
        if (selectionGeneration.current === generation) setActiveOperation(undefined);
      }
    },
    [
      attachRun,
      executeRecoverable,
      host,
      recoverPendingOperation,
      refreshRunControllability,
      selectedSession,
      sessionLoadState,
      updateRuns,
    ],
  );

  const interruptRun = useCallback(async () => {
    if (selectedSession === undefined) return;
    const active = runs.find((run) => ACTIVE_RUN_STATES.has(run.status));
    if (active === undefined) return;
    if (!controllableRunIds.has(active.runId)) {
      setNotice(
        checkedRunControlIds.has(active.runId)
          ? "This active run belongs to another process and is available as read-only history."
          : "Run control availability could not be confirmed. Refresh the conversation before retrying.",
      );
      return;
    }
    setActiveOperation("run.interrupt");
    try {
      const invocation = host.prepare({
        kind: "run.interrupt",
        input: {
          reason: "Interrupted from Starweaver Desktop",
          runId: active.runId,
          sessionId: selectedSession.sessionId,
        },
      });
      const result = await executeRecoverable(invocation);
      updateRuns((current) => replaceRun(current, result.run));
    } catch {
      setNotice(
        pendingOperations.current.size > 0
          ? "The interrupt outcome is unresolved. Retry recovery without sending it again."
          : "The active run could not be interrupted.",
      );
    } finally {
      setActiveOperation(undefined);
    }
  }, [
    checkedRunControlIds,
    controllableRunIds,
    executeRecoverable,
    host,
    runs,
    selectedSession,
    updateRuns,
  ]);

  const steerRun = useCallback(
    async (text: string) => {
      if (selectedSession === undefined) return;
      const active = runs.find((run) => ACTIVE_RUN_STATES.has(run.status));
      const direction = text.trim();
      if (active === undefined || direction.length === 0) return;
      if (!controllableRunIds.has(active.runId)) {
        setNotice(
          checkedRunControlIds.has(active.runId)
            ? "This active run belongs to another process and cannot be steered here."
            : "Run control availability could not be confirmed. Refresh the conversation before retrying.",
        );
        return;
      }
      if (Array.from(direction).length > 16_384) {
        setNotice("A run direction can contain at most 16,384 Unicode characters.");
        return;
      }
      setActiveOperation("run.steer");
      try {
        const invocation = host.prepare({
          kind: "run.steer",
          input: {
            runId: active.runId,
            sessionId: selectedSession.sessionId,
            text: direction,
          },
        });
        await executeRecoverable(invocation);
        setNotice("Direction sent to the active run.");
      } catch {
        setNotice(
          pendingOperations.current.size > 0
            ? "The direction outcome is unresolved. Retry recovery without sending it again."
            : "The direction could not be delivered to the active run.",
        );
      } finally {
        setActiveOperation(undefined);
      }
    },
    [checkedRunControlIds, controllableRunIds, executeRecoverable, host, runs, selectedSession],
  );

  const loadApprovalDetail = useCallback(
    async (approvalId: string, sessionId: string): Promise<ApprovalDetail | undefined> => {
      const existing = approvalDetails[approvalId];
      if (existing !== undefined) return existing;
      setApprovalDetailErrors((current) => {
        if (!current.has(approvalId)) return current;
        const next = new Set(current);
        next.delete(approvalId);
        return next;
      });
      try {
        const detail = (await host.approvalShow({ approvalId, sessionId })).approval;
        setApprovalDetails((current) => ({ ...current, [approvalId]: detail }));
        return detail;
      } catch {
        setApprovalDetailErrors((current) => new Set(current).add(approvalId));
        setNotice("This approval could not be opened from durable history.");
        return undefined;
      }
    },
    [approvalDetails, host],
  );

  const loadDeferredDetail = useCallback(
    async (deferredId: string, sessionId: string): Promise<DeferredDetail | undefined> => {
      const existing = deferredDetails[deferredId];
      if (existing !== undefined) return existing;
      setDeferredDetailErrors((current) => {
        if (!current.has(deferredId)) return current;
        const next = new Set(current);
        next.delete(deferredId);
        return next;
      });
      try {
        const detail = (await host.deferredShow({ deferredId, sessionId })).deferred;
        setDeferredDetails((current) => ({ ...current, [deferredId]: detail }));
        return detail;
      } catch {
        setDeferredDetailErrors((current) => new Set(current).add(deferredId));
        setNotice("This deferred request could not be opened from durable history.");
        return undefined;
      }
    },
    [deferredDetails, host],
  );

  const resumeInteractionRun = useCallback(
    async (sessionId: string, runId: string): Promise<RunResumeResult> => {
      const invocation = host.prepare({
        kind: "run.resume",
        input: {
          continuationMode: "preserve",
          runId,
          sessionId,
        },
      });
      const resumed = await executeRecoverable(invocation);
      if (selectedSession?.sessionId === sessionId) {
        const generation = selectionGeneration.current;
        const refreshed = await refreshSession(sessionId, generation);
        if (refreshed !== undefined) {
          void attachRun(sessionId, resumed.run.runId, generation, true);
        }
      }
      return resumed;
    },
    [attachRun, executeRecoverable, host, refreshSession, selectedSession],
  );

  const decideApproval = useCallback(
    async (
      approval: ApprovalSummary,
      decision: "approved" | "denied",
      reason?: string,
    ): Promise<boolean> => {
      setActiveOperation("approval.decide");
      setNotice(undefined);
      try {
        const invocation = host.prepare({
          kind: "approval.decide",
          input: {
            approvalId: approval.approvalId,
            decision,
            expectedRevision: approval.revision,
            sessionId: approval.sessionId,
            ...(reason?.trim() ? { reason: reason.trim() } : {}),
          },
        });
        const result = await executeRecoverable(invocation);
        setApprovals((current) =>
          replaceById(current, result.approval, (entry) => entry.approvalId),
        );
        try {
          await resumeInteractionRun(approval.sessionId, approval.runId);
          setNotice(decision === "approved" ? "Approved and resumed." : "Denied and resumed.");
        } catch {
          setNotice(
            "The decision is durable, but resume is still pending. Retry recovery or resume it from the Inbox.",
          );
        }
        await refreshInteractions();
        return true;
      } catch {
        setNotice(
          pendingOperations.current.size > 0
            ? "The decision outcome is unresolved. Retry recovery without deciding again."
            : "This approval changed before the decision could be applied.",
        );
        return false;
      } finally {
        setActiveOperation(undefined);
      }
    },
    [executeRecoverable, host, refreshInteractions, resumeInteractionRun],
  );

  const resolveClarification = useCallback(
    async (
      clarification: ClarificationSummary,
      answers: readonly ClarificationAnswer[],
      response?: string,
    ): Promise<boolean> => {
      setActiveOperation("clarification.resolve");
      setNotice(undefined);
      try {
        const invocation = host.prepare({
          kind: "clarification.resolve",
          input: {
            answers,
            clarificationId: clarification.clarificationId,
            expectedRevision: clarification.revision,
            sessionId: clarification.sessionId,
            ...(response?.trim() ? { response: response.trim() } : {}),
          },
        });
        const result = await executeRecoverable(invocation);
        setClarifications((current) =>
          replaceById(current, result.clarification, (entry) => entry.clarificationId),
        );
        try {
          await resumeInteractionRun(clarification.sessionId, clarification.runId);
          setNotice("Answers saved and the run resumed.");
        } catch {
          setNotice(
            "The answers are durable, but resume is still pending. Retry recovery or resume it from the Inbox.",
          );
        }
        await refreshInteractions();
        return true;
      } catch {
        setNotice(
          pendingOperations.current.size > 0
            ? "The answer outcome is unresolved. Retry recovery without submitting it again."
            : "This question changed before the answers could be applied.",
        );
        return false;
      } finally {
        setActiveOperation(undefined);
      }
    },
    [executeRecoverable, host, refreshInteractions, resumeInteractionRun],
  );

  const resolveDeferred = useCallback(
    async (
      record: DeferredSummary,
      outcome:
        | { readonly kind: "completed"; readonly text: string }
        | { readonly kind: "failed"; readonly error: string },
    ): Promise<boolean> => {
      setActiveOperation(outcome.kind === "completed" ? "deferred.complete" : "deferred.fail");
      setNotice(undefined);
      try {
        const result =
          outcome.kind === "completed"
            ? await executeRecoverable(
                host.prepare({
                  kind: "deferred.complete",
                  input: {
                    deferredId: record.deferredId,
                    expectedRevision: record.revision,
                    resultText: outcome.text,
                    sessionId: record.sessionId,
                  },
                }),
              )
            : await executeRecoverable(
                host.prepare({
                  kind: "deferred.fail",
                  input: {
                    deferredId: record.deferredId,
                    error: outcome.error,
                    expectedRevision: record.revision,
                    sessionId: record.sessionId,
                  },
                }),
              );
        setDeferred((current) =>
          replaceById(current, result.deferred, (entry) => entry.deferredId),
        );
        try {
          await resumeInteractionRun(record.sessionId, record.runId);
          setNotice("Deferred result saved and the run resumed.");
        } catch {
          setNotice(
            "The deferred result is durable, but resume is still pending. Retry recovery or resume it from the Inbox.",
          );
        }
        await refreshInteractions();
        return true;
      } catch {
        setNotice(
          pendingOperations.current.size > 0
            ? "The deferred outcome is unresolved. Retry recovery without submitting it again."
            : "This deferred request changed before the result could be applied.",
        );
        return false;
      } finally {
        setActiveOperation(undefined);
      }
    },
    [executeRecoverable, host, refreshInteractions, resumeInteractionRun],
  );

  const resumeResolvedInteraction = useCallback(
    async (sessionId: string, runId: string): Promise<boolean> => {
      setActiveOperation("run.resume");
      setNotice(undefined);
      try {
        await resumeInteractionRun(sessionId, runId);
        await refreshInteractions();
        setNotice("The waiting run resumed.");
        return true;
      } catch {
        setNotice(
          pendingOperations.current.size > 0
            ? "The resume outcome is unresolved. Retry recovery without resuming again."
            : "This run is not ready to resume.",
        );
        return false;
      } finally {
        setActiveOperation(undefined);
      }
    },
    [refreshInteractions, resumeInteractionRun],
  );

  const activeRun = runs.find((run) => ACTIVE_RUN_STATES.has(run.status));
  const activeRunControlKnown =
    activeRun !== undefined && checkedRunControlIds.has(activeRun.runId);
  const activeRunControllable = activeRun !== undefined && controllableRunIds.has(activeRun.runId);
  const selectedWorkspace = workspaces.find(
    (workspace) => workspace.workspaceId === selectedSession?.workspaceId,
  );
  const runTranscriptText = useMemo(
    () =>
      Object.fromEntries(
        Object.entries(transcripts).flatMap(([runId, transcript]) => {
          const text = transcriptText(transcript);
          return text === undefined ? [] : [[runId, text]];
        }),
      ) as Readonly<Record<string, string>>,
    [transcripts],
  );
  const { selectedProfile, profileReady, profileReadinessIssue } = deriveProfileReadiness(
    profiles,
    modelSelection,
    profileDetail,
  );
  const profileSelectionRecoveryPending = pendingOperationIds.some((operationId) => {
    const pending = pendingOperations.current.get(operationId);
    return pending?.invocation.operation.kind === "model.select";
  });
  const promptRecoveryPending =
    selectedSession !== undefined &&
    pendingOperationIds.some((operationId) => {
      const pending = pendingOperations.current.get(operationId);
      return (
        pending?.invocation.operation.kind === "run.start" &&
        pending.invocation.operation.input.sessionId === selectedSession.sessionId
      );
    });

  return {
    phase,
    workspaces,
    sessions,
    hasMoreSessions: sessionPageToken !== undefined,
    sessionsLoadingMore,
    selectedSession,
    selectedWorkspace,
    sessionLoadState,
    runs,
    hasMoreRuns: runPageToken !== undefined,
    runsLoadingMore,
    activeRun,
    activeRunControlKnown,
    activeRunControllable,
    conversationWindow: conversationSessionId !== undefined,
    approvals,
    clarifications,
    deferred,
    approvalDetails,
    approvalDetailErrors,
    deferredDetails,
    deferredDetailErrors,
    interactionsLoading,
    profiles,
    modelSelection,
    selectedProfile,
    profileDetail,
    profileReady,
    profileReadinessIssue,
    profileSelectionRecoveryPending,
    runtimeConfig,
    runtimeConfigStatus,
    runtimeConfigValidation,
    reloadCandidateEtag,
    settingsLoading,
    runTranscriptText,
    promptRecoveryPending,
    recoveryPending: pendingOperationIds.length > 0,
    busy,
    activeOperation,
    notice,
    createWorkspace,
    createSession,
    selectSession,
    loadMoreSessions,
    loadMoreRuns,
    sendPrompt,
    interruptRun,
    steerRun,
    decideApproval,
    resolveClarification,
    resolveDeferred,
    resumeResolvedInteraction,
    loadApprovalDetail,
    loadDeferredDetail,
    refreshInteractions,
    refreshSettings,
    selectProfile,
    validateRuntimeConfig,
    saveRuntimeConfig,
    previewRuntimeReload,
    commitRuntimeReload,
    discardStagedRuntimeConfig,
    retryPendingOperations: retryPendingProductOperations,
    retryCatalog: refreshCatalog,
  };
}
