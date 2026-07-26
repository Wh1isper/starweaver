import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { WorkspaceProduct } from "./WorkspaceProduct";

const productMocks = vi.hoisted(() => ({
  useWorkspaceProduct: vi.fn(),
}));
const desktopMocks = vi.hoisted(() => ({
  openConversationWindow: vi.fn(),
}));

vi.mock("../bridge/desktop", () => desktopMocks);
vi.mock("./useWorkspaceProduct", () => ({
  useWorkspaceProduct: productMocks.useWorkspaceProduct,
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
  status: "active" as const,
  title: "Desktop work",
  updatedAt: "2026-07-25T01:00:00Z",
  workspaceId: workspace.workspaceId,
};

function product(overrides: Record<string, unknown> = {}) {
  return {
    phase: "ready",
    workspaces: [workspace],
    sessions: [session],
    hasMoreSessions: false,
    sessionsLoadingMore: false,
    selectedSession: session,
    selectedWorkspace: workspace,
    sessionLoadState: { kind: "ready", sessionId: session.sessionId },
    runs: [],
    hasMoreRuns: false,
    runsLoadingMore: false,
    activeRun: undefined,
    activeRunControlKnown: true,
    activeRunControllable: true,
    conversationWindow: false,
    approvals: [],
    clarifications: [],
    deferred: [],
    approvalDetails: {},
    approvalDetailErrors: new Set<string>(),
    deferredDetails: {},
    deferredDetailErrors: new Set<string>(),
    interactionsLoading: false,
    profiles: [],
    modelSelection: undefined,
    selectedProfile: undefined,
    profileDetail: undefined,
    profileReady: false,
    profileReadinessIssue: "Profile catalog is unavailable.",
    profileSelectionRecoveryPending: false,
    runtimeConfig: undefined,
    runtimeConfigStatus: undefined,
    runtimeConfigValidation: undefined,
    reloadCandidateEtag: undefined,
    settingsLoading: false,
    runTranscriptText: {},
    promptRecoveryPending: false,
    recoveryPending: false,
    busy: false,
    activeOperation: undefined,
    notice: undefined,
    createWorkspace: vi.fn().mockResolvedValue(undefined),
    createSession: vi.fn().mockResolvedValue(undefined),
    selectSession: vi.fn().mockResolvedValue(undefined),
    loadMoreSessions: vi.fn().mockResolvedValue(undefined),
    loadMoreRuns: vi.fn().mockResolvedValue(undefined),
    sendPrompt: vi.fn().mockResolvedValue(true),
    interruptRun: vi.fn().mockResolvedValue(undefined),
    steerRun: vi.fn().mockResolvedValue(undefined),
    decideApproval: vi.fn().mockResolvedValue(true),
    resolveClarification: vi.fn().mockResolvedValue(true),
    resolveDeferred: vi.fn().mockResolvedValue(true),
    resumeResolvedInteraction: vi.fn().mockResolvedValue(true),
    loadApprovalDetail: vi.fn().mockResolvedValue(undefined),
    loadDeferredDetail: vi.fn().mockResolvedValue(undefined),
    refreshInteractions: vi.fn().mockResolvedValue(undefined),
    refreshSettings: vi.fn().mockResolvedValue(undefined),
    selectProfile: vi.fn().mockResolvedValue(true),
    validateRuntimeConfig: vi.fn().mockResolvedValue(undefined),
    saveRuntimeConfig: vi.fn().mockResolvedValue(true),
    previewRuntimeReload: vi.fn().mockResolvedValue(true),
    commitRuntimeReload: vi.fn().mockResolvedValue(true),
    discardStagedRuntimeConfig: vi.fn().mockResolvedValue(true),
    retryPendingOperations: vi.fn().mockResolvedValue(undefined),
    retryCatalog: vi.fn().mockResolvedValue(undefined),
    ...overrides,
  };
}

describe("WorkspaceProduct", () => {
  beforeEach(() => {
    productMocks.useWorkspaceProduct.mockReset();
    desktopMocks.openConversationWindow.mockReset();
    desktopMocks.openConversationWindow.mockResolvedValue({
      label: "conversation-test",
      reused: false,
      sessionId: session.sessionId,
    });
  });

  it("offers all three native workspace entry points without asking for a path", async () => {
    const state = product({
      selectedSession: undefined,
      selectedWorkspace: undefined,
      sessionLoadState: { kind: "idle" },
      sessions: [],
    });
    productMocks.useWorkspaceProduct.mockReturnValue(state);
    render(<WorkspaceProduct />);

    fireEvent.click(screen.getByRole("button", { name: /open folder/i }));
    await waitFor(() =>
      expect(state.createWorkspace).toHaveBeenCalledWith({ kind: "open_existing" }, undefined),
    );

    fireEvent.change(screen.getByPlaceholderText("my-project"), {
      target: { value: "new-project" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Create" }));
    await waitFor(() =>
      expect(state.createWorkspace).toHaveBeenCalledWith(
        { kind: "create_empty", name: "new-project" },
        "new-project",
      ),
    );

    fireEvent.click(screen.getByRole("button", { name: /start without a folder/i }));
    await waitFor(() =>
      expect(state.createWorkspace).toHaveBeenCalledWith({ kind: "managed" }, "Scratch workspace"),
    );
    expect(screen.queryByLabelText(/path/i)).not.toBeInTheDocument();
  });

  it("submits a valid workspace name through the keyboard form path", () => {
    const state = product({ selectedSession: undefined, sessionLoadState: { kind: "idle" } });
    productMocks.useWorkspaceProduct.mockReturnValue(state);
    render(<WorkspaceProduct />);

    const input = screen.getByRole("textbox", { name: /workspace folder name/i });
    fireEvent.change(input, { target: { value: "keyboard-project" } });
    const form = input.closest("form");
    if (form === null) throw new Error("workspace form is unavailable");
    fireEvent.submit(form);

    expect(state.createWorkspace).toHaveBeenCalledWith(
      { kind: "create_empty", name: "keyboard-project" },
      "keyboard-project",
    );
  });

  it("renders durable conversation history and starts a new prompt", async () => {
    const state = product({
      runs: [
        {
          createdAt: "2026-07-25T01:01:00Z",
          inputPreview: "Inspect this workspace",
          outputPreview: "The workspace is ready.",
          revision: "2",
          runId: "run-1",
          sessionId: session.sessionId,
          status: "completed",
          updatedAt: "2026-07-25T01:02:00Z",
        },
      ],
    });
    productMocks.useWorkspaceProduct.mockReturnValue(state);
    render(<WorkspaceProduct />);

    expect(screen.getByText("Inspect this workspace")).toBeInTheDocument();
    expect(screen.getByText("The workspace is ready.")).toBeInTheDocument();

    fireEvent.change(screen.getByLabelText("Message Starweaver"), {
      target: { value: "Continue with the next step" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Send" }));

    await waitFor(() =>
      expect(state.sendPrompt).toHaveBeenCalledWith("Continue with the next step"),
    );
  });

  it("offers a jump to latest control after the reader scrolls away", () => {
    const run = {
      createdAt: "2026-07-25T01:01:00Z",
      inputPreview: "Question",
      outputPreview: "Answer",
      revision: "1",
      runId: "run-scroll",
      sessionId: session.sessionId,
      status: "completed",
      updatedAt: "2026-07-25T01:02:00Z",
    };
    productMocks.useWorkspaceProduct.mockReturnValue(product({ runs: [run] }));
    render(<WorkspaceProduct />);

    const log = screen.getByRole("log", { name: /conversation messages/i });
    Object.defineProperties(log, {
      clientHeight: { configurable: true, value: 300 },
      scrollHeight: { configurable: true, value: 1000 },
      scrollTop: { configurable: true, value: 100, writable: true },
    });
    const scrollTo = vi.fn();
    Object.defineProperty(log, "scrollTo", { configurable: true, value: scrollTo });
    fireEvent.scroll(log);
    fireEvent.click(screen.getByRole("button", { name: /jump to latest/i }));

    expect(scrollTo).toHaveBeenCalledWith({ behavior: "smooth", top: 1000 });
  });

  it("does not present a false empty conversation while session state is loading", () => {
    productMocks.useWorkspaceProduct.mockReturnValue(
      product({
        sessionLoadState: { kind: "loading", sessionId: session.sessionId },
      }),
    );
    render(<WorkspaceProduct />);

    expect(screen.getByRole("heading", { name: "Opening conversation" })).toBeInTheDocument();
    expect(screen.queryByText("What are we working on?")).not.toBeInTheDocument();
    expect(screen.getByLabelText("Message Starweaver")).toBeDisabled();
  });

  it("keeps run submission disabled and retries a failed session hydration", async () => {
    const state = product({
      sessionLoadState: { kind: "error", sessionId: session.sessionId },
    });
    productMocks.useWorkspaceProduct.mockReturnValue(state);
    render(<WorkspaceProduct />);

    expect(
      screen.getByRole("heading", { name: "Conversation could not be opened" }),
    ).toBeInTheDocument();
    expect(screen.getByLabelText("Message Starweaver")).toBeDisabled();
    fireEvent.click(screen.getByRole("button", { name: "Try again" }));
    expect(state.selectSession).toHaveBeenCalledWith(session);
  });

  it("keeps prompt drafts scoped to their conversation", () => {
    const otherSession = {
      ...session,
      sessionId: "session-2",
      title: "Other work",
    };
    const first = product({ sessions: [session, otherSession] });
    productMocks.useWorkspaceProduct.mockReturnValue(first);
    const view = render(<WorkspaceProduct />);

    fireEvent.change(screen.getByLabelText("Message Starweaver"), {
      target: { value: "Draft for the first conversation" },
    });
    productMocks.useWorkspaceProduct.mockReturnValue(
      product({
        sessions: [session, otherSession],
        selectedSession: otherSession,
        sessionLoadState: { kind: "ready", sessionId: otherSession.sessionId },
      }),
    );
    view.rerender(<WorkspaceProduct />);
    expect(screen.getByLabelText("Message Starweaver")).toHaveValue("");

    fireEvent.change(screen.getByLabelText("Message Starweaver"), {
      target: { value: "Draft for the second conversation" },
    });
    productMocks.useWorkspaceProduct.mockReturnValue(first);
    view.rerender(<WorkspaceProduct />);
    expect(screen.getByLabelText("Message Starweaver")).toHaveValue(
      "Draft for the first conversation",
    );
  });

  it("retains the composer while a prompt outcome remains unresolved", async () => {
    const state = product({ sendPrompt: vi.fn().mockResolvedValue(false) });
    productMocks.useWorkspaceProduct.mockReturnValue(state);
    render(<WorkspaceProduct />);

    const composer = screen.getByLabelText("Message Starweaver");
    fireEvent.change(composer, { target: { value: "Keep this exact prompt" } });
    fireEvent.click(screen.getByRole("button", { name: "Send" }));

    await waitFor(() => expect(state.sendPrompt).toHaveBeenCalledWith("Keep this exact prompt"));
    expect(composer).toHaveValue("Keep this exact prompt");
  });

  it("offers an explicit retry for unresolved local mutations", async () => {
    const state = product({ recoveryPending: true });
    productMocks.useWorkspaceProduct.mockReturnValue(state);
    render(<WorkspaceProduct />);

    fireEvent.click(screen.getByRole("button", { name: "Retry recovery" }));

    await waitFor(() => expect(state.retryPendingOperations).toHaveBeenCalledOnce());
  });

  it("traps modal drawer focus, closes on Escape, and restores the trigger", async () => {
    productMocks.useWorkspaceProduct.mockReturnValue(product());
    render(<WorkspaceProduct />);

    const trigger = screen.getByRole("button", { name: /open settings/i });
    trigger.focus();
    fireEvent.click(trigger);
    const dialog = screen.getByRole("dialog", { name: "Settings" });
    const close = screen.getByRole("button", { name: /close settings/i });
    await waitFor(() => expect(close).toHaveFocus());
    expect(dialog).toHaveAttribute("aria-modal", "true");
    expect(screen.getByLabelText(/workspace and conversation navigation/i)).toHaveAttribute(
      "inert",
    );

    fireEvent.keyDown(dialog, { key: "Escape" });
    expect(screen.queryByRole("dialog", { name: "Settings" })).not.toBeInTheDocument();
    expect(trigger).toHaveFocus();
  });

  it("opens typed settings and saves a reviewed runtime draft", async () => {
    const runtimeConfig = {
      defaultProfile: "general",
      profiles: [
        {
          instructions: [],
          modelId: "codex:gpt-5.6-sol",
          name: "general",
          toolsets: ["filesystem"],
        },
        {
          instructions: ["Focus on implementation."],
          modelId: "codex:gpt-5.6-sol",
          name: "coding",
          toolsets: ["filesystem"],
        },
      ],
      providers: [{ enabled: true, name: "codex" }],
    };
    const state = product({
      profiles: [
        { label: "General", modelId: "codex:gpt-5.6-sol", name: "general", source: "builtin" },
      ],
      modelSelection: {
        modelId: "codex:gpt-5.6-sol",
        revision: "1",
        selectedProfile: "general",
      },
      selectedProfile: {
        label: "General",
        modelId: "codex:gpt-5.6-sol",
        name: "general",
        source: "builtin",
      },
      profileDetail: {
        instructions: [],
        label: "General",
        mcpServers: [],
        modelId: "codex:gpt-5.6-sol",
        name: "general",
        subagents: [],
        toolsets: ["filesystem"],
      },
      profileReady: true,
      profileReadinessIssue: undefined,
      runtimeConfig,
      runtimeConfigStatus: {
        active: { etag: "etag-1", generation: "1", materializationDigest: "sha256:active" },
        desired: { etag: "etag-1", generation: "1", materializationDigest: "sha256:active" },
        restartRequired: false,
      },
    });
    productMocks.useWorkspaceProduct.mockReturnValue(state);
    render(<WorkspaceProduct />);

    fireEvent.click(screen.getByRole("button", { name: "Open settings" }));
    expect(screen.getByRole("dialog", { name: "Settings" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Default profile" }));
    expect(screen.getByText("Ready for new runs")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Profile details" }));
    fireEvent.change(screen.getByLabelText("Profile to edit"), {
      target: { value: "coding" },
    });
    fireEvent.change(screen.getByLabelText("Model ID"), {
      target: { value: "codex:gpt-5.6-mini" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Save changes" }));

    await waitFor(() =>
      expect(state.saveRuntimeConfig).toHaveBeenCalledWith({
        ...runtimeConfig,
        profiles: [
          runtimeConfig.profiles[0],
          {
            ...runtimeConfig.profiles[1],
            modelId: "codex:gpt-5.6-mini",
          },
        ],
      }),
    );
  });

  it("updates closed Desktop preferences without mixing them into runtime config", async () => {
    const state = product();
    const desktopPreferences = {
      snapshot: {
        schemaVersion: 1 as const,
        revision: "3",
        preferences: {
          theme: "system" as const,
          density: "comfortable" as const,
          windowCloseBehavior: "keep_running" as const,
        },
      },
      loading: false,
      saving: false,
      issue: undefined,
      recoveryPending: false,
      save: vi.fn().mockResolvedValue(true),
      retryPending: vi.fn().mockResolvedValue(true),
      reload: vi.fn().mockResolvedValue(undefined),
    };
    productMocks.useWorkspaceProduct.mockReturnValue(state);
    render(<WorkspaceProduct desktopPreferences={desktopPreferences} />);

    fireEvent.click(screen.getByRole("button", { name: "Open settings" }));
    fireEvent.change(screen.getByLabelText("Theme"), { target: { value: "dark" } });

    await waitFor(() =>
      expect(desktopPreferences.save).toHaveBeenCalledWith({
        theme: "dark",
        density: "comfortable",
        windowCloseBehavior: "keep_running",
      }),
    );
    expect(state.saveRuntimeConfig).not.toHaveBeenCalled();
  });

  it("opens a durable clarification from the interaction inbox", async () => {
    const clarification = {
      clarificationId: "clarification-1",
      questions: [
        {
          header: "Scope",
          multiSelect: false,
          options: [
            { description: "Keep the current scope", label: "Minimal" },
            { description: "Include adjacent cleanup", label: "Broader" },
          ],
          question: "How broad should the change be?",
        },
      ],
      revision: "1",
      runId: "run-waiting",
      sessionId: session.sessionId,
      status: "pending" as const,
      updatedAt: "2026-07-25T01:02:00Z",
    };
    const state = product({ clarifications: [clarification] });
    productMocks.useWorkspaceProduct.mockReturnValue(state);
    render(<WorkspaceProduct />);

    fireEvent.click(screen.getByRole("button", { name: /interaction inbox, 1 pending/i }));
    expect(screen.getByRole("dialog", { name: /needs your attention/i })).toBeInTheDocument();
    expect(screen.getByText("How broad should the change be?")).toBeInTheDocument();

    fireEvent.click(screen.getByLabelText(/Minimal/));
    fireEvent.click(screen.getByRole("button", { name: "Answer and continue" }));
    await waitFor(() =>
      expect(state.resolveClarification).toHaveBeenCalledWith(clarification, [
        {
          question: "How broad should the change be?",
          selectedOptions: ["Minimal"],
        },
      ]),
    );
  });

  it("clears deferred drafts when switching between durable requests", () => {
    const deferredA = {
      deferredId: "deferred-a",
      revision: "1",
      runId: "run-a",
      sessionId: session.sessionId,
      status: "pending" as const,
      toolName: "Deferred A",
      updatedAt: "2026-07-25T01:04:00Z",
    };
    const deferredB = {
      ...deferredA,
      deferredId: "deferred-b",
      runId: "run-b",
      toolName: "Deferred B",
      updatedAt: "2026-07-25T01:03:00Z",
    };
    productMocks.useWorkspaceProduct.mockReturnValue(
      product({
        deferred: [deferredA, deferredB],
        deferredDetails: {
          "deferred-a": {
            requestComplete: true,
            requestJson: '{"request":"a"}',
            summary: deferredA,
          },
          "deferred-b": {
            requestComplete: true,
            requestJson: '{"request":"b"}',
            summary: deferredB,
          },
        },
      }),
    );
    render(<WorkspaceProduct />);

    fireEvent.click(screen.getByRole("button", { name: /interaction inbox, 2 pending/i }));
    fireEvent.change(screen.getByLabelText("Result"), { target: { value: "result for A" } });
    fireEvent.change(screen.getByLabelText("Failure reason"), {
      target: { value: "failure for A" },
    });
    fireEvent.click(screen.getByRole("button", { name: /Deferred B/i }));

    expect(screen.getByLabelText("Result")).toHaveValue("");
    expect(screen.getByLabelText("Failure reason")).toHaveValue("");
    expect(screen.getByText('{"request":"b"}')).toBeInTheDocument();
  });

  it("does not offer resume while the waiting run still has a pending interaction", () => {
    const waitingRun = {
      createdAt: "2026-07-25T01:00:00Z",
      revision: "2",
      runId: "run-waiting",
      sessionId: session.sessionId,
      status: "waiting" as const,
      updatedAt: "2026-07-25T01:05:00Z",
    };
    const approval = {
      approvalId: "approval-pending",
      revision: "1",
      runId: waitingRun.runId,
      sessionId: session.sessionId,
      status: "pending" as const,
      title: "Current request",
      updatedAt: "2026-07-25T01:05:00Z",
    };
    productMocks.useWorkspaceProduct.mockReturnValue(
      product({
        runs: [waitingRun],
        approvals: [
          approval,
          {
            ...approval,
            approvalId: "approval-old",
            status: "denied",
            updatedAt: "2026-07-25T01:04:00Z",
          },
        ],
      }),
    );
    render(<WorkspaceProduct />);

    fireEvent.click(screen.getByRole("button", { name: /interaction inbox, 1 pending/i }));

    expect(screen.queryByRole("button", { name: "Resume run" })).not.toBeInTheDocument();
  });

  it("selects the first interaction when durable discovery finishes after the inbox opens", () => {
    const initial = product({ interactionsLoading: true });
    productMocks.useWorkspaceProduct.mockReturnValue(initial);
    const view = render(<WorkspaceProduct />);

    fireEvent.click(screen.getByRole("button", { name: /interaction inbox, 0 pending/i }));
    expect(screen.getByText("All clear")).toBeInTheDocument();

    productMocks.useWorkspaceProduct.mockReturnValue(
      product({
        approvals: [
          {
            approvalId: "approval-after-open",
            revision: "1",
            runId: "run-waiting",
            sessionId: session.sessionId,
            status: "pending",
            title: "Review generated changes",
            updatedAt: "2026-07-25T01:03:00Z",
          },
        ],
      }),
    );
    view.rerender(<WorkspaceProduct />);

    expect(screen.getByRole("heading", { name: "Review generated changes" })).toBeInTheDocument();
    expect(screen.queryByText("This request changed")).not.toBeInTheDocument();
  });

  it("groups sessions with a missing live workspace under history only", () => {
    const unavailable = {
      ...session,
      sessionId: "session-history-only",
      title: "Unavailable workspace conversation",
      workspaceId: "workspace-revoked",
    };
    productMocks.useWorkspaceProduct.mockReturnValue(
      product({
        sessions: [session, unavailable],
      }),
    );

    render(<WorkspaceProduct />);

    expect(screen.getByText("History only")).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: /Unavailable workspace conversation/ }),
    ).toBeInTheDocument();
  });

  it("opens one backend-routed window for the selected conversation", async () => {
    productMocks.useWorkspaceProduct.mockReturnValue(product());
    render(<WorkspaceProduct />);

    fireEvent.click(screen.getByRole("button", { name: "Open in new window" }));

    await waitFor(() =>
      expect(desktopMocks.openConversationWindow).toHaveBeenCalledWith(session.sessionId),
    );
  });

  it("renders a conversation window without workspace or settings navigation", () => {
    productMocks.useWorkspaceProduct.mockReturnValue(product({ conversationWindow: true }));
    render(
      <WorkspaceProduct windowRoute={{ kind: "conversation", sessionId: session.sessionId }} />,
    );

    expect(
      screen.queryByRole("complementary", { name: "Workspace and conversation navigation" }),
    ).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Open in new window" })).not.toBeInTheDocument();
    expect(screen.queryByLabelText("Default profile for new runs")).not.toBeInTheDocument();
  });

  it("shows a foreign active run as read-only", () => {
    const activeRun = {
      createdAt: "2026-07-25T01:01:00Z",
      revision: "1",
      runId: "run-foreign",
      sessionId: session.sessionId,
      status: "running" as const,
      updatedAt: "2026-07-25T01:01:00Z",
    };
    productMocks.useWorkspaceProduct.mockReturnValue(
      product({
        activeRun,
        activeRunControlKnown: true,
        activeRunControllable: false,
        runs: [activeRun],
      }),
    );

    render(<WorkspaceProduct />);

    expect(screen.getByText("active elsewhere")).toBeInTheDocument();
    expect(screen.getByLabelText("Steer active run")).toBeDisabled();
    expect(screen.queryByRole("button", { name: "Stop" })).not.toBeInTheDocument();
  });

  it("steers and interrupts the active run from the same composer", async () => {
    const activeRun = {
      createdAt: "2026-07-25T01:01:00Z",
      inputPreview: "Make the change",
      revision: "1",
      runId: "run-active",
      sessionId: session.sessionId,
      status: "running" as const,
      updatedAt: "2026-07-25T01:01:00Z",
    };
    const state = product({ runs: [activeRun], activeRun });
    productMocks.useWorkspaceProduct.mockReturnValue(state);
    render(<WorkspaceProduct />);

    fireEvent.change(screen.getByLabelText("Steer active run"), {
      target: { value: "Keep the API minimal" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Steer" }));
    await waitFor(() => expect(state.steerRun).toHaveBeenCalledWith("Keep the API minimal"));

    fireEvent.click(screen.getByRole("button", { name: "Stop" }));
    await waitFor(() => expect(state.interruptRun).toHaveBeenCalledOnce());
  });
});
