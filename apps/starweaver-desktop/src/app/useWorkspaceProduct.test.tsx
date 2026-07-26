import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { useWorkspaceProduct } from "./useWorkspaceProduct";

const hostMocks = vi.hoisted(() => ({
  execute: vi.fn(),
  approvalDecide: vi.fn(),
  approvalList: vi.fn(),
  approvalShow: vi.fn(),
  catalogList: vi.fn(),
  clarificationList: vi.fn(),
  clarificationResolve: vi.fn(),
  deferredComplete: vi.fn(),
  deferredFail: vi.fn(),
  deferredList: vi.fn(),
  deferredShow: vi.fn(),
  configActivate: vi.fn(),
  configDiscard: vi.fn(),
  configGet: vi.fn(),
  configReload: vi.fn(),
  configUpdate: vi.fn(),
  configValidate: vi.fn(),
  modelSelect: vi.fn(),
  profileGet: vi.fn(),
  pendingOperations: vi.fn(),
  retryAcknowledgement: vi.fn(),
  runInterrupt: vi.fn(),
  runList: vi.fn(),
  runResume: vi.fn(),
  runStart: vi.fn(),
  runStatus: vi.fn(),
  runSteer: vi.fn(),
  sessionCreate: vi.fn(),
  sessionGet: vi.fn(),
  sessionList: vi.fn(),
  subscribe: vi.fn(),
  workspaceList: vi.fn(),
}));
const desktopMocks = vi.hoisted(() => ({
  registerDesktopWorkspace: vi.fn(),
}));

vi.mock("../generated/host/client", () => {
  class DesktopHostExecutionError extends Error {
    constructor(
      readonly invocation: { operationId: string; operation: { kind: string; input: unknown } },
      readonly cause: unknown,
    ) {
      super("unresolved execution");
    }
  }
  class DesktopHostAcknowledgementError extends Error {
    constructor(readonly invocation: unknown) {
      super("unresolved acknowledgement");
    }
  }
  return {
    DesktopHostExecutionError,
    DesktopHostAcknowledgementError,
    DesktopHostClient: class {
      prepare(operation: { kind: string; input: unknown }) {
        return { operationId: "desktop-op-v1-test", operation };
      }
      async execute(invocation: {
        operationId: string;
        operation: { kind: string; input: unknown };
      }) {
        hostMocks.execute(invocation);
        try {
          switch (invocation.operation.kind) {
            case "approval.decide":
              return await hostMocks.approvalDecide(invocation.operation.input);
            case "clarification.resolve":
              return await hostMocks.clarificationResolve(invocation.operation.input);
            case "deferred.complete":
              return await hostMocks.deferredComplete(invocation.operation.input);
            case "deferred.fail":
              return await hostMocks.deferredFail(invocation.operation.input);
            case "model.select":
              return await hostMocks.modelSelect(invocation.operation.input);
            case "config.update":
              return await hostMocks.configUpdate(invocation.operation.input);
            case "config.reload":
              return await hostMocks.configReload(invocation.operation.input);
            case "config.activate":
              return await hostMocks.configActivate(invocation.operation.input);
            case "config.discard":
              return await hostMocks.configDiscard(invocation.operation.input);
            case "run.resume":
              return await hostMocks.runResume(invocation.operation.input);
            case "run.start":
              return await hostMocks.runStart(invocation.operation.input);
            case "run.steer":
              return await hostMocks.runSteer(invocation.operation.input);
            case "run.interrupt":
              return await hostMocks.runInterrupt(invocation.operation.input);
            case "session.create":
              return await hostMocks.sessionCreate(invocation.operation.input);
            default:
              throw new Error(`unexpected operation: ${invocation.operation.kind}`);
          }
        } catch (error: unknown) {
          if (
            error instanceof DesktopHostExecutionError ||
            error instanceof DesktopHostAcknowledgementError
          ) {
            throw error;
          }
          throw new DesktopHostExecutionError(invocation, error);
        }
      }
      approvalList(input: unknown) {
        return hostMocks.approvalList(input);
      }
      approvalShow(input: unknown) {
        return hostMocks.approvalShow(input);
      }
      catalogList(input: unknown) {
        return hostMocks.catalogList(input);
      }
      clarificationList(input: unknown) {
        return hostMocks.clarificationList(input);
      }
      deferredList(input: unknown) {
        return hostMocks.deferredList(input);
      }
      deferredShow(input: unknown) {
        return hostMocks.deferredShow(input);
      }
      configGet(input: unknown) {
        return hostMocks.configGet(input);
      }
      configValidate(input: unknown) {
        return hostMocks.configValidate(input);
      }
      profileGet(input: unknown) {
        return hostMocks.profileGet(input);
      }
      pendingOperations() {
        return hostMocks.pendingOperations();
      }
      retryAcknowledgement(failure: unknown) {
        return hostMocks.retryAcknowledgement(failure);
      }
      runInterrupt(input: unknown) {
        return hostMocks.runInterrupt(input);
      }
      runList(input: unknown) {
        return hostMocks.runList(input);
      }
      runStatus(input: unknown) {
        return hostMocks.runStatus(input);
      }
      runSteer(input: unknown) {
        return hostMocks.runSteer(input);
      }
      sessionCreate(input: unknown) {
        return hostMocks.sessionCreate(input);
      }
      sessionGet(input: unknown) {
        return hostMocks.sessionGet(input);
      }
      sessionList(input: unknown) {
        return hostMocks.sessionList(input);
      }
      subscribe(input: unknown, handler: (event: unknown) => void) {
        return hostMocks.subscribe(input, handler);
      }
      workspaceList(input: unknown) {
        return hostMocks.workspaceList(input);
      }
    },
  };
});
vi.mock("../bridge/desktop", () => ({
  executeDesktopWorkspaceRegistration: desktopMocks.registerDesktopWorkspace,
}));

const workspace = {
  displayLabel: "Starweaver",
  provenanceDigest: "sha256:workspace",
  revision: "1",
  state: "active" as const,
  workspaceId: "workspace-1",
};
const session = {
  createdAt: "2026-07-25T00:00:00Z",
  revision: "1",
  sessionId: "session-1",
  status: "active",
  title: "Desktop work",
  updatedAt: "2026-07-25T01:00:00Z",
  workspaceId: workspace.workspaceId,
};
const completedRun = {
  createdAt: "2026-07-25T01:01:00Z",
  inputPreview: "Inspect this workspace",
  outputPreview: "The workspace is ready.",
  revision: "2",
  runId: "run-1",
  sessionId: session.sessionId,
  status: "completed",
  updatedAt: "2026-07-25T01:02:00Z",
};

function catalog(runs: readonly Record<string, unknown>[] = [completedRun]) {
  hostMocks.workspaceList.mockResolvedValue({
    page: { hasMore: false },
    workspaces: [workspace],
  });
  hostMocks.sessionList.mockResolvedValue({
    page: { hasMore: false },
    sessions: [session],
  });
  hostMocks.sessionGet.mockResolvedValue({ runs, session });
}

function pendingSubscription() {
  let handler: ((event: unknown) => void) | undefined;
  let complete: (() => void) | undefined;
  const close = vi.fn().mockResolvedValue(undefined);
  const done = new Promise<void>((resolve) => {
    complete = resolve;
  });
  hostMocks.subscribe.mockImplementation(
    async (_input: unknown, nextHandler: (event: unknown) => void) => {
      handler = nextHandler;
      return {
        caughtUp: Promise.resolve(),
        close,
        done,
        token: "desktop-host-subscription-safe",
      };
    },
  );
  return {
    close,
    complete() {
      if (complete === undefined) throw new Error("subscription completion is unavailable");
      complete();
    },
    event(value: unknown) {
      if (handler === undefined) throw new Error("subscription handler is unavailable");
      handler(value);
    },
  };
}

describe("useWorkspaceProduct", () => {
  beforeEach(() => {
    for (const mock of Object.values(hostMocks)) mock.mockReset();
    desktopMocks.registerDesktopWorkspace.mockReset();
    hostMocks.approvalList.mockResolvedValue({ approvals: [], page: { hasMore: false } });
    hostMocks.clarificationList.mockResolvedValue({
      clarifications: [],
      page: { hasMore: false },
    });
    hostMocks.deferredList.mockResolvedValue({ deferred: [], page: { hasMore: false } });
    hostMocks.catalogList.mockResolvedValue({
      profiles: [
        { label: "General", modelId: "codex:gpt-5.6-sol", name: "general", source: "builtin" },
      ],
      selection: {
        modelId: "codex:gpt-5.6-sol",
        revision: "1",
        selectedProfile: "general",
      },
    });
    hostMocks.profileGet.mockResolvedValue({
      profile: {
        instructions: [],
        label: "General",
        mcpServers: [],
        modelId: "codex:gpt-5.6-sol",
        name: "general",
        subagents: [],
        toolsets: ["filesystem"],
      },
    });
    hostMocks.configGet.mockResolvedValue({
      config: {
        defaultProfile: "general",
        profiles: [
          {
            instructions: [],
            modelId: "codex:gpt-5.6-sol",
            name: "general",
            toolsets: ["filesystem"],
          },
        ],
        providers: [{ enabled: true, name: "codex" }],
      },
      status: {
        active: { etag: "etag-1", generation: "1", materializationDigest: "sha256:active" },
        desired: { etag: "etag-1", generation: "1", materializationDigest: "sha256:active" },
        restartRequired: false,
      },
    });
    hostMocks.configValidate.mockResolvedValue({
      validation: {
        candidateFingerprint: "sha256:candidate",
        changedCategories: [],
        issues: [],
        restartRequired: false,
        valid: true,
      },
    });
    hostMocks.pendingOperations.mockResolvedValue([]);
    hostMocks.runStatus.mockResolvedValue({
      controllableByCurrentHost: true,
      run: completedRun,
    });
  });

  it("reconstructs durable history after loading the local catalog", async () => {
    catalog();

    const { result } = renderHook(() => useWorkspaceProduct());

    await waitFor(() => expect(result.current.phase).toBe("ready"));
    await waitFor(() => expect(result.current.runs).toEqual([completedRun]));
    expect(result.current.selectedSession?.sessionId).toBe(session.sessionId);
    expect(result.current.selectedWorkspace?.workspaceId).toBe(workspace.workspaceId);
    expect(hostMocks.sessionGet).toHaveBeenCalledWith({ sessionId: session.sessionId });
  });

  it("suspends live subscriptions while hidden and reconnects when visible", async () => {
    const activeRun = { ...completedRun, runId: "run-visible", status: "running" };
    catalog([activeRun]);
    hostMocks.runStatus.mockResolvedValue({ controllableByCurrentHost: true, run: activeRun });
    const closes: ReturnType<typeof vi.fn>[] = [];
    hostMocks.subscribe.mockImplementation(async () => {
      const close = vi.fn().mockResolvedValue(undefined);
      closes.push(close);
      return {
        caughtUp: Promise.resolve(),
        close,
        done: new Promise<void>(() => undefined),
        token: `desktop-host-subscription-${closes.length}`,
      };
    });
    const originalVisibility = Object.getOwnPropertyDescriptor(document, "visibilityState");
    let visibility: DocumentVisibilityState = "visible";
    Object.defineProperty(document, "visibilityState", {
      configurable: true,
      get: () => visibility,
    });

    try {
      renderHook(() => useWorkspaceProduct());
      await waitFor(() => expect(hostMocks.subscribe).toHaveBeenCalledTimes(1));

      visibility = "hidden";
      document.dispatchEvent(new Event("visibilitychange"));
      await waitFor(() => expect(closes[0]).toHaveBeenCalled());

      visibility = "visible";
      document.dispatchEvent(new Event("visibilitychange"));
      await waitFor(() => expect(hostMocks.subscribe).toHaveBeenCalledTimes(2));
      expect(hostMocks.sessionGet.mock.calls.length).toBeGreaterThanOrEqual(2);
    } finally {
      if (originalVisibility === undefined)
        delete (document as { visibilityState?: string }).visibilityState;
      else Object.defineProperty(document, "visibilityState", originalVisibility);
    }
  });

  it("loads older runs through stable Desktop page tokens", async () => {
    const allRuns = Array.from({ length: 101 }, (_, index) => ({
      ...completedRun,
      runId: `run-${index + 1}`,
      revision: String(index + 1),
    }));
    catalog(allRuns.slice(1));
    hostMocks.runList
      .mockResolvedValueOnce({
        page: { hasMore: true, nextPageToken: "desktop-page-v1-older" },
        runs: [...allRuns.slice(1)].reverse(),
      })
      .mockResolvedValueOnce({
        page: { hasMore: false },
        runs: [allRuns[0]],
      });
    hostMocks.subscribe.mockImplementation(async () => ({
      caughtUp: Promise.resolve(),
      close: vi.fn().mockResolvedValue(undefined),
      done: Promise.resolve(),
      token: "desktop-host-subscription-safe",
    }));

    const { result } = renderHook(() => useWorkspaceProduct());

    await waitFor(() => expect(result.current.hasMoreRuns).toBe(true));
    expect(result.current.runs[0]?.runId).toBe("run-2");
    await act(async () => {
      await result.current.loadMoreRuns();
    });

    expect(result.current.hasMoreRuns).toBe(false);
    expect(result.current.runs).toHaveLength(101);
    expect(result.current.runs[0]?.runId).toBe("run-1");
    expect(hostMocks.runList).toHaveBeenNthCalledWith(2, {
      pageToken: "desktop-page-v1-older",
      sessionId: session.sessionId,
    });
  });

  it("materializes profile readiness and persists a default for future runs", async () => {
    catalog();
    hostMocks.catalogList.mockResolvedValue({
      profiles: [
        { label: "General", modelId: "codex:gpt-5.6-sol", name: "general", source: "builtin" },
        { label: "Research", modelId: "codex:gpt-5.6-sol", name: "research", source: "user" },
      ],
      selection: {
        modelId: "codex:gpt-5.6-sol",
        revision: "1",
        selectedProfile: "general",
      },
    });
    hostMocks.modelSelect.mockResolvedValue({
      receipt: {},
      selection: {
        modelId: "codex:gpt-5.6-sol",
        revision: "2",
        selectedProfile: "research",
      },
    });
    hostMocks.profileGet.mockImplementation(async ({ name }: { name: string }) => ({
      profile: {
        instructions: [],
        label: name === "research" ? "Research" : "General",
        mcpServers: [],
        modelId: "codex:gpt-5.6-sol",
        name,
        subagents: [],
        toolsets: name === "research" ? ["filesystem", "search"] : ["filesystem"],
      },
    }));

    const { result } = renderHook(() => useWorkspaceProduct());
    await waitFor(() => expect(result.current.profileReady).toBe(true));

    await act(async () => {
      expect(await result.current.selectProfile("research")).toBe(true);
    });

    expect(hostMocks.modelSelect).toHaveBeenCalledWith({ profile: "research" });
    expect(result.current.modelSelection?.selectedProfile).toBe("research");
    expect(result.current.profileDetail?.toolsets).toEqual(["filesystem", "search"]);
  });

  it("fences a newer profile choice behind exact recovery of an unresolved selection", async () => {
    catalog();
    hostMocks.catalogList.mockResolvedValue({
      profiles: [
        { label: "General", modelId: "codex:gpt-5.6-sol", name: "general", source: "builtin" },
        { label: "Research", modelId: "codex:gpt-5.6-sol", name: "research", source: "user" },
      ],
      selection: {
        modelId: "codex:gpt-5.6-sol",
        revision: "1",
        selectedProfile: "general",
      },
    });
    hostMocks.modelSelect.mockRejectedValue(new Error("transport outcome unknown"));
    const { result } = renderHook(() => useWorkspaceProduct());
    await waitFor(() => expect(result.current.profileReady).toBe(true));

    await act(async () => {
      expect(await result.current.selectProfile("research")).toBe(false);
    });
    expect(result.current.profileSelectionRecoveryPending).toBe(true);

    await act(async () => {
      expect(await result.current.selectProfile("general")).toBe(false);
    });
    expect(hostMocks.modelSelect).toHaveBeenCalledTimes(1);
    expect(result.current.notice).toMatch(/recover the unresolved profile change/i);
  });

  it("does not report a stale selected model identity as ready", async () => {
    catalog();
    hostMocks.catalogList.mockResolvedValue({
      profiles: [
        { label: "General", modelId: "codex:gpt-5.6-next", name: "general", source: "user" },
      ],
      selection: {
        modelId: "codex:gpt-5.6-sol",
        revision: "1",
        selectedProfile: "general",
      },
    });
    hostMocks.profileGet.mockResolvedValue({
      profile: {
        instructions: [],
        label: "General",
        mcpServers: [],
        modelId: "codex:gpt-5.6-next",
        name: "general",
        subagents: [],
        toolsets: ["filesystem"],
      },
    });

    const { result } = renderHook(() => useWorkspaceProduct());
    await waitFor(() => expect(result.current.profileReadinessIssue).toMatch(/stale/i));
    expect(result.current.profileReady).toBe(false);
  });

  it("validates and executes one exact runtime config update", async () => {
    catalog();
    hostMocks.configUpdate.mockResolvedValue({
      receipt: {},
      status: {
        active: { etag: "etag-2", generation: "2", materializationDigest: "sha256:next" },
        desired: { etag: "etag-2", generation: "2", materializationDigest: "sha256:next" },
        restartRequired: false,
      },
      validation: {
        candidateFingerprint: "sha256:candidate",
        changedCategories: ["profiles"],
        issues: [],
        restartRequired: false,
        valid: true,
      },
    });
    const { result } = renderHook(() => useWorkspaceProduct());
    await waitFor(() => expect(result.current.runtimeConfig).toBeDefined());
    const currentConfig = result.current.runtimeConfig;
    if (currentConfig === undefined) throw new Error("runtime config was not loaded");
    const candidate = {
      ...currentConfig,
      profiles: currentConfig.profiles.map((profile) => ({
        ...profile,
        instructions: ["Be concise"],
      })),
    };

    await act(async () => {
      expect(await result.current.saveRuntimeConfig(candidate)).toBe(true);
    });

    expect(hostMocks.configValidate).toHaveBeenCalledWith({ candidate });
    expect(hostMocks.configUpdate).toHaveBeenCalledWith({
      candidate,
      expectedActiveEtag: "etag-1",
    });
    expect(
      hostMocks.execute.mock.calls.filter(
        ([invocation]) => invocation.operation.kind === "config.update",
      ),
    ).toHaveLength(1);
  });

  it("reduces durable run and output events into the active conversation", async () => {
    const runningRun = { ...completedRun, outputPreview: undefined, status: "running" };
    catalog([runningRun]);
    const subscription = pendingSubscription();
    const { result } = renderHook(() => useWorkspaceProduct());
    await waitFor(() => expect(hostMocks.subscribe).toHaveBeenCalledOnce());

    act(() => {
      subscription.event({
        delivery: {
          record: {
            event: {
              kind: "transcript_changed",
              runId: runningRun.runId,
              transcriptSequence: "1",
              update: {
                delta: "A live ",
                kind: "text_appended",
                messageId: "assistant:run-1:turn-1:part-0",
              },
            },
          },
        },
      });
      subscription.event({
        delivery: {
          record: {
            event: {
              kind: "transcript_changed",
              runId: runningRun.runId,
              transcriptSequence: "2",
              update: {
                delta: "response",
                kind: "text_appended",
                messageId: "assistant:run-1:turn-1:part-0",
              },
            },
          },
        },
      });
    });
    expect(result.current.runs[0]?.outputPreview).toBeUndefined();
    expect(result.current.runTranscriptText[runningRun.runId]).toBe("A live response");

    act(() => {
      subscription.event({
        delivery: {
          record: {
            event: {
              kind: "transcript_changed",
              runId: runningRun.runId,
              transcriptSequence: "2",
              update: {
                delta: " duplicate",
                kind: "text_appended",
                messageId: "assistant:run-1:turn-1:part-0",
              },
            },
          },
        },
      });
      subscription.event({
        delivery: {
          record: {
            event: {
              kind: "transcript_changed",
              runId: runningRun.runId,
              transcriptSequence: "1",
              update: {
                delta: " stale",
                kind: "text_appended",
                messageId: "assistant:run-1:turn-1:part-0",
              },
            },
          },
        },
      });
    });
    expect(result.current.runTranscriptText[runningRun.runId]).toBe("A live response");

    act(() => {
      subscription.event({
        delivery: {
          record: {
            event: {
              kind: "output_available",
              preview: "A durable partial response",
              runId: runningRun.runId,
            },
          },
        },
      });
    });
    expect(result.current.runs[0]?.outputPreview).toBe("A durable partial response");
    expect(result.current.runTranscriptText[runningRun.runId]).toBe("A live response");

    const completed = {
      ...runningRun,
      inputPreview: undefined,
      outputPreview: undefined,
      revision: "3",
      status: "completed",
    };
    act(() => {
      subscription.event({
        delivery: { record: { event: { kind: "run_changed", run: completed } } },
      });
    });
    expect(result.current.runs[0]).toEqual({
      ...completed,
      inputPreview: completedRun.inputPreview,
      outputPreview: "A durable partial response",
    });
    expect(result.current.runTranscriptText[runningRun.runId]).toBe("A live response");
  });

  it("does not let an older same-session refresh replace a newly started run", async () => {
    const runningRun = { ...completedRun, status: "running" };
    catalog([runningRun]);
    const subscription = pendingSubscription();
    const { result } = renderHook(() => useWorkspaceProduct());
    await waitFor(() => expect(result.current.sessionLoadState.kind).toBe("ready"));
    await waitFor(() => expect(hostMocks.subscribe).toHaveBeenCalledOnce());

    const terminal = { ...runningRun, revision: "3", status: "completed" };
    act(() => {
      subscription.event({
        delivery: { record: { event: { kind: "run_changed", run: terminal } } },
      });
    });
    expect(result.current.activeRun).toBeUndefined();

    let resolveStaleRefresh:
      | ((value: { runs: (typeof terminal)[]; session: typeof session }) => void)
      | undefined;
    hostMocks.sessionGet.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          resolveStaleRefresh = resolve;
        }),
    );
    act(() => subscription.complete());
    await waitFor(() => expect(hostMocks.sessionGet).toHaveBeenCalledTimes(2));

    const startedRun = {
      ...runningRun,
      createdAt: "2026-07-25T01:03:00Z",
      inputPreview: "Start newer work",
      revision: "1",
      runId: "run-2",
      status: "queued",
      updatedAt: "2026-07-25T01:03:00Z",
    };
    hostMocks.runStart.mockResolvedValue({ run: startedRun });
    hostMocks.runStatus.mockResolvedValue({
      controllableByCurrentHost: true,
      run: startedRun,
    });
    hostMocks.subscribe.mockImplementationOnce(async () => ({
      caughtUp: Promise.resolve(),
      close: vi.fn().mockResolvedValue(undefined),
      done: new Promise<void>(() => undefined),
      token: "desktop-host-subscription-new-run",
    }));

    await act(async () => {
      expect(await result.current.sendPrompt("Start newer work")).toBe(true);
    });
    expect(result.current.runs.some((run) => run.runId === startedRun.runId)).toBe(true);

    act(() => {
      if (resolveStaleRefresh === undefined) throw new Error("stale refresh did not start");
      resolveStaleRefresh({ runs: [terminal], session });
    });
    await act(async () => Promise.resolve());

    expect(result.current.runs.some((run) => run.runId === startedRun.runId)).toBe(true);
  });

  it("retries run-control discovery when replay advances authoritative run state", async () => {
    catalog([]);
    const subscription = pendingSubscription();
    const startedRun = {
      ...completedRun,
      createdAt: "2026-07-25T01:03:00Z",
      inputPreview: "Start controlled work",
      revision: "1",
      runId: "run-controlled",
      status: "queued",
      updatedAt: "2026-07-25T01:03:00Z",
    };
    hostMocks.runStart.mockResolvedValue({ run: startedRun });
    let resolveFirstStatus:
      | ((value: { controllableByCurrentHost: boolean; run: typeof startedRun }) => void)
      | undefined;
    hostMocks.runStatus
      .mockImplementationOnce(
        () =>
          new Promise((resolve) => {
            resolveFirstStatus = resolve;
          }),
      )
      .mockResolvedValue({ controllableByCurrentHost: true, run: startedRun });

    const { result } = renderHook(() => useWorkspaceProduct());
    await waitFor(() => expect(result.current.sessionLoadState.kind).toBe("ready"));
    await act(async () => {
      expect(await result.current.sendPrompt("Start controlled work")).toBe(true);
    });
    await waitFor(() => expect(hostMocks.subscribe).toHaveBeenCalledOnce());

    act(() => {
      subscription.event({
        delivery: {
          record: {
            event: {
              kind: "run_changed",
              run: { ...startedRun, revision: "2", status: "running" },
            },
          },
        },
      });
    });
    act(() => {
      if (resolveFirstStatus === undefined) throw new Error("first run status did not start");
      resolveFirstStatus({ controllableByCurrentHost: true, run: startedRun });
    });

    await waitFor(() => expect(hostMocks.runStatus).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(result.current.activeRunControllable).toBe(true));
  });

  it("rebuilds a terminal transcript from durable origin after renderer remount", async () => {
    catalog([{ ...completedRun, outputPreview: "bounded preview" }]);
    hostMocks.subscribe.mockImplementation(
      async (
        _input: unknown,
        handler: (event: { delivery: { record: { event: Record<string, unknown> } } }) => void,
      ) => {
        handler({
          delivery: {
            record: {
              event: {
                kind: "transcript_changed",
                runId: completedRun.runId,
                transcriptSequence: "1",
                update: {
                  kind: "message_started",
                  messageId: "assistant:run-1:turn-1:part-0",
                },
              },
            },
          },
        });
        handler({
          delivery: {
            record: {
              event: {
                kind: "transcript_changed",
                runId: completedRun.runId,
                transcriptSequence: "2",
                update: {
                  delta: "full durable answer",
                  kind: "text_appended",
                  messageId: "assistant:run-1:turn-1:part-0",
                },
              },
            },
          },
        });
        handler({
          delivery: {
            record: {
              event: {
                kind: "transcript_changed",
                runId: completedRun.runId,
                transcriptSequence: "3",
                update: {
                  kind: "message_finished",
                  messageId: "assistant:run-1:turn-1:part-0",
                },
              },
            },
          },
        });
        return {
          caughtUp: Promise.resolve(),
          close: vi.fn().mockResolvedValue(undefined),
          done: Promise.resolve(),
          token: "desktop-host-subscription-terminal",
        };
      },
    );

    const first = renderHook(() => useWorkspaceProduct());
    await waitFor(() =>
      expect(first.result.current.runTranscriptText[completedRun.runId]).toBe(
        "full durable answer",
      ),
    );
    first.unmount();

    const second = renderHook(() => useWorkspaceProduct());
    await waitFor(() =>
      expect(second.result.current.runTranscriptText[completedRun.runId]).toBe(
        "full durable answer",
      ),
    );
    expect(hostMocks.subscribe).toHaveBeenCalledTimes(2);
    second.unmount();
  });

  it("hydrates more terminal runs than the bounded subscription concurrency", async () => {
    const terminalRuns = Array.from({ length: 6 }, (_, index) => ({
      ...completedRun,
      runId: `run-${index + 1}`,
      outputPreview: `preview-${index + 1}`,
    }));
    catalog(terminalRuns);
    let activeSubscriptions = 0;
    let maximumSubscriptions = 0;
    hostMocks.subscribe.mockImplementation(
      async (input: { runId: string; sessionId: string }, handler: (event: unknown) => void) => {
        activeSubscriptions += 1;
        maximumSubscriptions = Math.max(maximumSubscriptions, activeSubscriptions);
        handler({
          delivery: {
            record: {
              event: {
                kind: "transcript_changed",
                runId: input.runId,
                transcriptSequence: "1",
                update: {
                  delta: `transcript-${input.runId}`,
                  kind: "text_appended",
                  messageId: `assistant:${input.runId}:turn-1:part-0`,
                },
              },
            },
          },
        });
        let finishReplay!: () => void;
        const caughtUp = new Promise<void>((resolve) => {
          finishReplay = resolve;
        });
        let complete!: () => void;
        const done = new Promise<void>((resolve) => {
          complete = resolve;
        });
        queueMicrotask(finishReplay);
        const close = vi.fn().mockImplementation(async () => {
          activeSubscriptions -= 1;
          complete();
        });
        return {
          caughtUp,
          close,
          done,
          token: `desktop-host-subscription-${input.runId}`,
        };
      },
    );

    const { result } = renderHook(() => useWorkspaceProduct());

    await waitFor(() => expect(hostMocks.subscribe).toHaveBeenCalledTimes(6));
    await waitFor(() =>
      expect(Object.keys(result.current.runTranscriptText)).toHaveLength(terminalRuns.length),
    );
    expect(maximumSubscriptions).toBeLessThanOrEqual(4);
    expect(result.current.runTranscriptText["run-6"]).toBe("transcript-run-6");
  });

  it("discovers a durable clarification and resolves it before explicit resume", async () => {
    const waitingRun = { ...completedRun, outputPreview: undefined, status: "waiting" };
    const resumedRun = {
      ...completedRun,
      outputPreview: undefined,
      runId: "run-resumed",
      status: "running",
    };
    const clarification = {
      clarificationId: "clarification-1",
      questions: [
        {
          header: "Scope",
          multiSelect: false,
          options: [
            { description: "Keep it focused", label: "Minimal" },
            { description: "Include cleanup", label: "Broader" },
          ],
          question: "How broad should the change be?",
        },
      ],
      revision: "1",
      runId: waitingRun.runId,
      sessionId: session.sessionId,
      status: "pending" as const,
      updatedAt: "2026-07-25T01:02:00Z",
    };
    catalog([waitingRun]);
    pendingSubscription();
    hostMocks.clarificationList.mockResolvedValue({
      clarifications: [clarification],
      page: { hasMore: false },
    });
    hostMocks.clarificationResolve.mockResolvedValue({
      clarification: { ...clarification, revision: "2", status: "resolved" },
      receipt: {},
    });
    hostMocks.runResume.mockResolvedValue({
      receipt: {},
      run: resumedRun,
      sourceRunId: waitingRun.runId,
    });
    hostMocks.sessionGet
      .mockResolvedValueOnce({ runs: [waitingRun], session })
      .mockResolvedValue({ runs: [{ ...waitingRun, status: "completed" }, resumedRun], session });

    const { result } = renderHook(() => useWorkspaceProduct());
    await waitFor(() => expect(result.current.clarifications).toEqual([clarification]));

    await act(async () => {
      expect(
        await result.current.resolveClarification(clarification, [
          {
            question: "How broad should the change be?",
            selectedOptions: ["Minimal"],
          },
        ]),
      ).toBe(true);
    });

    const operationKinds = hostMocks.execute.mock.calls.map(
      ([invocation]) => invocation.operation.kind,
    );
    expect(operationKinds).toContain("clarification.resolve");
    expect(operationKinds).toContain("run.resume");
    expect(operationKinds.indexOf("clarification.resolve")).toBeLessThan(
      operationKinds.indexOf("run.resume"),
    );
    expect(hostMocks.runResume).toHaveBeenCalledWith({
      continuationMode: "preserve",
      runId: waitingRun.runId,
      sessionId: session.sessionId,
    });
  });

  it("reconciles a durable pending start with its exact invocation on remount", async () => {
    const runningRun = { ...completedRun, outputPreview: undefined, status: "running" };
    catalog([runningRun]);
    pendingSubscription();
    const invocation = {
      operationId: "desktop-op-v1-persisted",
      operation: {
        kind: "run.start",
        input: {
          continuationMode: "preserve",
          input: [{ kind: "text", text: "Persisted prompt" }],
          sessionId: session.sessionId,
        },
      },
    };
    hostMocks.pendingOperations.mockResolvedValue([invocation]);
    hostMocks.runStart.mockResolvedValue({ receipt: {}, run: runningRun });

    const { result } = renderHook(() => useWorkspaceProduct());

    await waitFor(() => expect(hostMocks.execute).toHaveBeenCalledWith(invocation));
    await waitFor(() => expect(result.current.promptRecoveryPending).toBe(false));
    expect(hostMocks.runStart).toHaveBeenCalledOnce();
    expect(result.current.activeRun?.runId).toBe(runningRun.runId);
  });

  it("stops ordered startup recovery before a later dependent mutation", async () => {
    catalog([]);
    const start = {
      operationId: "desktop-op-v1-start",
      operation: {
        kind: "run.start",
        input: {
          continuationMode: "preserve",
          input: [{ kind: "text", text: "First mutation" }],
          sessionId: session.sessionId,
        },
      },
    };
    const selection = {
      operationId: "desktop-op-v1-selection",
      operation: {
        kind: "model.select",
        input: { profile: "general" },
      },
    };
    hostMocks.pendingOperations.mockResolvedValue([start, selection]);
    hostMocks.runStart.mockRejectedValue(new Error("still unresolved"));
    hostMocks.modelSelect.mockResolvedValue({ receipt: {}, selection: {} });

    const { result } = renderHook(() => useWorkspaceProduct());
    await waitFor(() => expect(result.current.phase).toBe("ready"));
    await waitFor(() => expect(hostMocks.runStart).toHaveBeenCalledOnce());

    expect(hostMocks.execute).toHaveBeenCalledWith(start);
    expect(hostMocks.execute).not.toHaveBeenCalledWith(selection);
    expect(hostMocks.modelSelect).not.toHaveBeenCalled();
    expect(result.current.recoveryPending).toBe(true);
  });

  it("retries an unresolved start with the exact invocation", async () => {
    catalog([]);
    pendingSubscription();
    const runningRun = { ...completedRun, outputPreview: undefined, status: "running" };
    hostMocks.runStart
      .mockRejectedValueOnce(new Error("transport outcome unknown"))
      .mockResolvedValueOnce({ receipt: {}, run: runningRun });
    const { result } = renderHook(() => useWorkspaceProduct());
    await waitFor(() => expect(result.current.selectedSession?.sessionId).toBe(session.sessionId));

    await act(async () => {
      expect(await result.current.sendPrompt("Make the change")).toBe(false);
    });
    expect(result.current.promptRecoveryPending).toBe(true);
    expect(result.current.notice).toMatch(/same prompt/i);

    await act(async () => {
      expect(await result.current.sendPrompt("Make the change")).toBe(true);
    });
    expect(result.current.promptRecoveryPending).toBe(false);
    expect(hostMocks.execute).toHaveBeenCalledTimes(2);
    expect(hostMocks.execute.mock.calls[1]?.[0]).toBe(hostMocks.execute.mock.calls[0]?.[0]);
    expect(hostMocks.runStart).toHaveBeenCalledTimes(2);
    expect(result.current.activeRun?.runId).toBe(runningRun.runId);
  });

  it("recovers an unresolved conversation creation without allocating a new operation", async () => {
    catalog([]);
    hostMocks.sessionCreate
      .mockRejectedValueOnce(new Error("transport outcome unknown"))
      .mockResolvedValueOnce({ receipt: {}, session });
    const { result } = renderHook(() => useWorkspaceProduct());
    await waitFor(() => expect(result.current.phase).toBe("ready"));

    await act(async () => result.current.createSession(workspace));
    expect(result.current.recoveryPending).toBe(true);
    const firstInvocation = hostMocks.execute.mock.calls.find(
      ([invocation]) => invocation.operation.kind === "session.create",
    )?.[0];
    expect(firstInvocation).toBeDefined();

    await act(async () => result.current.retryPendingOperations());
    const sessionCreateInvocations = hostMocks.execute.mock.calls
      .map(([invocation]) => invocation)
      .filter((invocation) => invocation.operation.kind === "session.create");
    expect(sessionCreateInvocations).toHaveLength(2);
    expect(sessionCreateInvocations[1]).toBe(firstInvocation);
    expect(result.current.recoveryPending).toBe(false);
  });

  it("rejects an oversized Unicode prompt before preparing a durable mutation", async () => {
    catalog([]);
    const { result } = renderHook(() => useWorkspaceProduct());
    await waitFor(() => expect(result.current.selectedSession?.sessionId).toBe(session.sessionId));

    await act(async () => {
      expect(await result.current.sendPrompt("界".repeat(65_537))).toBe(false);
    });

    expect(hostMocks.runStart).not.toHaveBeenCalled();
    expect(result.current.recoveryPending).toBe(false);
    expect(result.current.notice).toMatch(/65,536 Unicode characters/);
  });

  it("does not attach a late run start to a newly selected session", async () => {
    catalog([]);
    pendingSubscription();
    const otherSession = {
      ...session,
      sessionId: "session-2",
      status: "active" as const,
      title: "Other work",
      updatedAt: "2026-07-25T02:00:00Z",
    };
    const otherRun = {
      ...completedRun,
      runId: "run-other",
      sessionId: otherSession.sessionId,
    };
    hostMocks.sessionGet.mockImplementation(async ({ sessionId }: { sessionId: string }) =>
      sessionId === otherSession.sessionId
        ? { runs: [otherRun], session: otherSession }
        : { runs: [], session },
    );
    let resolveStart: ((value: unknown) => void) | undefined;
    hostMocks.runStart.mockReturnValue(
      new Promise((resolve) => {
        resolveStart = resolve;
      }),
    );
    const lateRun = { ...completedRun, runId: "run-late", status: "running" as const };
    const { result } = renderHook(() => useWorkspaceProduct());
    await waitFor(() => expect(result.current.selectedSession?.sessionId).toBe(session.sessionId));

    let start: Promise<boolean> | undefined;
    act(() => {
      start = result.current.sendPrompt("Late prompt");
    });
    await waitFor(() => expect(hostMocks.runStart).toHaveBeenCalledOnce());
    await act(async () => result.current.selectSession(otherSession));
    expect(result.current.runs).toEqual([otherRun]);

    let lateOutcome: boolean | undefined;
    await act(async () => {
      resolveStart?.({ receipt: {}, run: lateRun });
      lateOutcome = await start;
    });
    expect(lateOutcome).toBe(false);
    expect(result.current.selectedSession?.sessionId).toBe(otherSession.sessionId);
    expect(result.current.runs).toEqual([otherRun]);
    expect(hostMocks.subscribe).not.toHaveBeenCalledWith(
      { sessionId: session.sessionId, runId: lateRun.runId },
      expect.any(Function),
    );
  });

  it("starts, steers, and interrupts one active run with typed host intents", async () => {
    catalog([]);
    const subscription = pendingSubscription();
    const runningRun = { ...completedRun, outputPreview: undefined, status: "running" };
    hostMocks.runStart.mockResolvedValue({ receipt: {}, run: runningRun });
    hostMocks.runSteer.mockResolvedValue({ accepted: true, receipt: {} });
    hostMocks.runInterrupt.mockResolvedValue({
      receipt: {},
      run: { ...runningRun, status: "cancelled" },
    });
    const { result } = renderHook(() => useWorkspaceProduct());
    await waitFor(() => expect(result.current.selectedSession?.sessionId).toBe(session.sessionId));

    await act(async () => result.current.sendPrompt("  Make the change  "));
    expect(hostMocks.runStart).toHaveBeenCalledWith({
      continuationMode: "preserve",
      input: [{ kind: "text", text: "Make the change" }],
      sessionId: session.sessionId,
    });
    await waitFor(() => expect(result.current.activeRun?.runId).toBe(runningRun.runId));

    await act(async () => result.current.steerRun("  Keep the API minimal  "));
    expect(hostMocks.runSteer).toHaveBeenCalledWith({
      runId: runningRun.runId,
      sessionId: session.sessionId,
      text: "Keep the API minimal",
    });

    await act(async () => result.current.interruptRun());
    expect(hostMocks.runInterrupt).toHaveBeenCalledWith({
      reason: "Interrupted from Starweaver Desktop",
      runId: runningRun.runId,
      sessionId: session.sessionId,
    });
    expect(result.current.activeRun).toBeUndefined();
    expect(subscription.close).not.toHaveBeenCalled();
  });

  it("loads session history incrementally with the opaque next-page token", async () => {
    const olderSession = {
      ...session,
      sessionId: "session-older",
      title: "Older conversation",
      updatedAt: "2026-07-24T01:00:00Z",
    };
    hostMocks.workspaceList.mockResolvedValue({
      page: { hasMore: false },
      workspaces: [workspace],
    });
    hostMocks.sessionList
      .mockResolvedValueOnce({
        page: { hasMore: true, nextPageToken: "page-token-2" },
        sessions: [session],
      })
      .mockResolvedValueOnce({
        page: { hasMore: false },
        sessions: [olderSession],
      });
    hostMocks.sessionGet.mockResolvedValue({ runs: [completedRun], session });

    const { result } = renderHook(() => useWorkspaceProduct());
    await waitFor(() => expect(result.current.phase).toBe("ready"));
    expect(result.current.sessions).toEqual([session]);
    expect(result.current.hasMoreSessions).toBe(true);

    await act(async () => result.current.loadMoreSessions());

    expect(hostMocks.sessionList).toHaveBeenLastCalledWith({ pageToken: "page-token-2" });
    expect(result.current.sessions).toEqual([session, olderSession]);
    expect(result.current.hasMoreSessions).toBe(false);
  });

  it("loads a backend-routed conversation without broad catalog or settings authority", async () => {
    hostMocks.workspaceList.mockResolvedValue({
      page: { hasMore: false },
      workspaces: [workspace],
    });
    hostMocks.sessionGet.mockResolvedValue({ runs: [completedRun], session });

    const { result } = renderHook(() =>
      useWorkspaceProduct({ conversationSessionId: session.sessionId }),
    );

    await waitFor(() => expect(result.current.phase).toBe("ready"));
    await waitFor(() => expect(result.current.selectedSession?.sessionId).toBe(session.sessionId));
    expect(result.current.conversationWindow).toBe(true);
    expect(hostMocks.sessionList).not.toHaveBeenCalled();
    expect(hostMocks.catalogList).not.toHaveBeenCalled();
    expect(hostMocks.configGet).not.toHaveBeenCalled();
    expect(hostMocks.approvalList).toHaveBeenCalledWith({
      sessionId: session.sessionId,
      state: "unresolved",
    });
    expect(hostMocks.clarificationList).toHaveBeenCalledWith({
      sessionId: session.sessionId,
      state: "unresolved",
    });
    expect(hostMocks.deferredList).toHaveBeenCalledWith({
      sessionId: session.sessionId,
      state: "unresolved",
    });
  });

  it("keeps a foreign active run readable but rejects local control", async () => {
    const runningRun = { ...completedRun, outputPreview: undefined, status: "running" as const };
    catalog([runningRun]);
    pendingSubscription();
    hostMocks.runStatus.mockResolvedValue({
      controllableByCurrentHost: false,
      run: runningRun,
    });

    const { result } = renderHook(() => useWorkspaceProduct());
    await waitFor(() => expect(result.current.activeRun?.runId).toBe(runningRun.runId));
    await waitFor(() => expect(result.current.activeRunControllable).toBe(false));

    await act(async () => result.current.steerRun("Do not send"));
    await act(async () => result.current.interruptRun());

    expect(hostMocks.runSteer).not.toHaveBeenCalled();
    expect(hostMocks.runInterrupt).not.toHaveBeenCalled();
    expect(result.current.notice).toMatch(/another process/i);
  });
});
