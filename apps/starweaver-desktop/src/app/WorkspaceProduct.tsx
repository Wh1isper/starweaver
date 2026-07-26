import type { FormEvent, KeyboardEvent } from "react";
import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { openConversationWindow } from "../bridge/desktop";
import type { DesktopWindowRoute } from "../bridge/types";
import { InteractionInbox, interactionInboxCount } from "./InteractionInbox";
import { SettingsPanel } from "./SettingsPanel";
import type { DesktopPreferencesState } from "./useDesktopPreferences";
import type { SessionSummary, WorkspaceSummary } from "./useWorkspaceProduct";
import { useWorkspaceProduct } from "./useWorkspaceProduct";

function workspaceName(workspace: WorkspaceSummary | undefined): string {
  return workspace?.displayLabel?.trim() || "Local workspace";
}

function sessionName(session: SessionSummary): string {
  return session.title?.trim() || "Untitled conversation";
}

function shortTime(value: string): string {
  const date = new Date(value);
  if (Number.isNaN(date.valueOf())) return "";
  return new Intl.DateTimeFormat(undefined, {
    hour: "numeric",
    minute: "2-digit",
    month: "short",
    day: "numeric",
  }).format(date);
}

function scrollToLatest(element: HTMLDivElement, behavior: ScrollBehavior = "auto") {
  if (typeof element.scrollTo === "function") {
    element.scrollTo({ behavior, top: element.scrollHeight });
  } else {
    element.scrollTop = element.scrollHeight;
  }
}

function operationStatus(operation: string): string {
  switch (operation) {
    case "workspace.register":
      return "Setting up workspace…";
    case "session.create":
      return "Creating conversation…";
    case "run.start":
      return "Starting run…";
    case "run.steer":
      return "Sending direction…";
    case "run.interrupt":
      return "Stopping run…";
    case "run.resume":
      return "Resuming run…";
    case "approval.decide":
      return "Saving decision…";
    case "clarification.resolve":
      return "Saving answers…";
    case "deferred.complete":
    case "deferred.fail":
      return "Saving deferred result…";
    case "model.select":
      return "Changing default profile…";
    case "config.update":
      return "Saving runtime settings…";
    case "config.reload":
      return "Reloading runtime settings…";
    case "config.activate":
      return "Activating runtime settings…";
    case "config.discard":
      return "Discarding staged settings…";
    default:
      return "Recovering the exact pending operation…";
  }
}

function StartCenter({
  busy,
  workspaces,
  onCreateWorkspace,
  onCreateSession,
}: {
  busy: boolean;
  workspaces: readonly WorkspaceSummary[];
  onCreateWorkspace: (
    intent:
      | { readonly kind: "open_existing" }
      | { readonly kind: "create_empty"; readonly name: string }
      | { readonly kind: "managed" },
    label?: string,
  ) => Promise<void>;
  onCreateSession: (workspace: WorkspaceSummary) => Promise<void>;
}) {
  const [name, setName] = useState("");
  const validName = name.trim() === name && name.length > 0 && name.length <= 80;

  return (
    <div className="start-center">
      <div className="start-intro">
        <p className="eyebrow">Local by design</p>
        <h1>Where should we begin?</h1>
        <p>
          Choose a folder you already use, create a clean workspace, or start in a retained private
          workspace managed by Starweaver.
        </p>
      </div>

      <fieldset className="start-actions">
        <legend className="sr-only">Workspace choices</legend>
        <button
          type="button"
          className="start-action"
          disabled={busy}
          onClick={() => void onCreateWorkspace({ kind: "open_existing" })}
        >
          <span className="start-action-mark">01</span>
          <strong>Open folder</strong>
          <small>Work in an existing local project</small>
        </button>

        <form
          className="start-action start-action-form"
          onSubmit={(event) => {
            event.preventDefault();
            if (validName && !busy) {
              void onCreateWorkspace({ kind: "create_empty", name }, name);
            }
          }}
        >
          <span className="start-action-mark">02</span>
          <strong>Create workspace</strong>
          <small>Choose a parent folder and create an empty private directory</small>
          <div className="workspace-name-row">
            <label className="sr-only" htmlFor="workspace-name">
              Workspace folder name
            </label>
            <input
              id="workspace-name"
              value={name}
              maxLength={80}
              placeholder="my-project"
              disabled={busy}
              onChange={(event) => setName(event.currentTarget.value)}
            />
            <button type="submit" disabled={busy || !validName}>
              Create
            </button>
          </div>
        </form>

        <button
          type="button"
          className="start-action"
          disabled={busy}
          onClick={() => void onCreateWorkspace({ kind: "managed" }, "Scratch workspace")}
        >
          <span className="start-action-mark">03</span>
          <strong>Start without a folder</strong>
          <small>Use a retained workspace in Starweaver application data</small>
        </button>
      </fieldset>

      {workspaces.length > 0 ? (
        <section className="existing-workspaces" aria-labelledby="existing-workspaces-title">
          <div>
            <p className="eyebrow">Already registered</p>
            <h2 id="existing-workspaces-title">Continue in a workspace</h2>
          </div>
          <div className="existing-workspace-list">
            {workspaces.map((workspace) => (
              <button
                type="button"
                key={workspace.workspaceId}
                disabled={busy}
                onClick={() => void onCreateSession(workspace)}
              >
                <span>{workspaceName(workspace)}</span>
                <small>New conversation</small>
              </button>
            ))}
          </div>
        </section>
      ) : null}
    </div>
  );
}

export function WorkspaceProduct({
  desktopPreferences,
  windowRoute = { kind: "main" },
}: {
  desktopPreferences?: DesktopPreferencesState;
  windowRoute?: DesktopWindowRoute;
} = {}) {
  const conversationWindow = windowRoute.kind === "conversation";
  const product = useWorkspaceProduct({
    conversationSessionId: windowRoute.kind === "conversation" ? windowRoute.sessionId : undefined,
  });
  const [showStartCenter, setShowStartCenter] = useState(false);
  const [showInteractionInbox, setShowInteractionInbox] = useState(false);
  const [showSettings, setShowSettings] = useState(false);
  const [composerDrafts, setComposerDrafts] = useState<Readonly<Record<string, string>>>({});
  const conversationScrollRef = useRef<HTMLDivElement>(null);
  const followLatest = useRef(true);
  const [showJumpLatest, setShowJumpLatest] = useState(false);
  const modalOpen = showInteractionInbox || showSettings;
  const composerKey = product.selectedSession?.sessionId ?? "new-conversation";
  const composer = composerDrafts[composerKey] ?? "";
  const setComposer = (value: string) => {
    setComposerDrafts((current) => ({ ...current, [composerKey]: value }));
  };
  const sessionsByWorkspace = useMemo(() => {
    const grouped = new Map<string, SessionSummary[]>();
    const availableWorkspaceIds = new Set(
      product.workspaces.map((workspace) => workspace.workspaceId),
    );
    for (const session of product.sessions) {
      const key =
        typeof session.workspaceId === "string" && availableWorkspaceIds.has(session.workspaceId)
          ? session.workspaceId
          : "history-only";
      const entries = grouped.get(key) ?? [];
      entries.push(session);
      grouped.set(key, entries);
    }
    return grouped;
  }, [product.sessions, product.workspaces]);

  useLayoutEffect(() => {
    if (
      product.selectedSession?.sessionId === undefined ||
      product.sessionLoadState.kind !== "ready"
    )
      return;
    const scroll = conversationScrollRef.current;
    if (scroll === null) return;
    followLatest.current = true;
    setShowJumpLatest(false);
    scrollToLatest(scroll);
  }, [product.selectedSession?.sessionId, product.sessionLoadState.kind]);

  useEffect(() => {
    const hasConversationContent =
      product.runs.length > 0 || Object.keys(product.runTranscriptText).length > 0;
    const scroll = conversationScrollRef.current;
    if (!hasConversationContent || scroll === null || product.sessionLoadState.kind !== "ready")
      return;
    if (followLatest.current) scrollToLatest(scroll);
    else setShowJumpLatest(true);
  }, [product.runTranscriptText, product.runs, product.sessionLoadState.kind]);

  if (product.phase === "loading") {
    return (
      <main className="product-loading" aria-live="polite">
        <div className="product-loading-mark" aria-hidden="true" />
        <p>Opening local workspaces and history…</p>
      </main>
    );
  }

  if (product.phase === "error") {
    return (
      <main className="product-loading" role="alert">
        <h1>Local history could not be opened</h1>
        <p>The runtime is connected, but its workspace catalog is temporarily unavailable.</p>
        <button type="button" onClick={() => void product.retryCatalog()}>
          Try again
        </button>
      </main>
    );
  }

  const useLauncher =
    !conversationWindow && (showStartCenter || product.selectedSession === undefined);
  const workspaceAvailable = product.selectedWorkspace !== undefined;
  const active = product.activeRun;
  const waiting = active?.status === "waiting";
  const sessionReady =
    product.selectedSession !== undefined &&
    product.sessionLoadState.kind === "ready" &&
    product.sessionLoadState.sessionId === product.selectedSession.sessionId;

  const submit = async (event?: FormEvent) => {
    event?.preventDefault();
    const text = composer.trim();
    if (
      text.length === 0 ||
      product.busy ||
      !sessionReady ||
      waiting ||
      (active !== undefined && !product.activeRunControllable)
    )
      return;
    if (active === undefined) {
      if (await product.sendPrompt(text)) setComposer("");
    } else {
      await product.steerRun(text);
      setComposer("");
    }
  };

  const composerKeyDown = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    if (event.key === "Enter" && !event.shiftKey && !event.nativeEvent.isComposing) {
      event.preventDefault();
      void submit();
    }
  };

  return (
    <main className={conversationWindow ? "product-shell conversation-window" : "product-shell"}>
      {!conversationWindow ? (
        <aside
          className="product-sidebar"
          aria-label="Workspace and conversation navigation"
          aria-hidden={modalOpen ? "true" : undefined}
          inert={modalOpen ? true : undefined}
        >
          <div className="sidebar-heading">
            <div>
              <span>Local</span>
              <strong>Workspaces</strong>
            </div>
            <div className="sidebar-heading-actions">
              <button
                type="button"
                className="settings-trigger"
                aria-label="Open settings"
                title="Profiles and runtime settings"
                onClick={() => setShowSettings(true)}
              >
                Settings
              </button>
              <button
                type="button"
                className={
                  interactionInboxCount(product) > 0
                    ? "inbox-trigger inbox-trigger-active"
                    : "inbox-trigger"
                }
                aria-label={`Open interaction inbox, ${interactionInboxCount(product)} pending`}
                title="Interaction inbox"
                onClick={() => setShowInteractionInbox(true)}
              >
                <span aria-hidden="true">Inbox</span>
                {interactionInboxCount(product) > 0 ? (
                  <b>{interactionInboxCount(product)}</b>
                ) : null}
              </button>
              <button
                type="button"
                aria-label="Open workspace start center"
                title="New workspace or conversation"
                onClick={() => setShowStartCenter(true)}
              >
                +
              </button>
            </div>
          </div>

          <nav className="session-navigation" aria-label="Conversation history">
            {product.workspaces.map((workspace) => {
              const workspaceSessions = sessionsByWorkspace.get(workspace.workspaceId) ?? [];
              return (
                <section key={workspace.workspaceId}>
                  <div className="workspace-nav-heading">
                    <span className="workspace-dot" aria-hidden="true" />
                    <span>{workspaceName(workspace)}</span>
                  </div>
                  {workspaceSessions.length === 0 ? (
                    <button
                      type="button"
                      className="empty-workspace-link"
                      onClick={() => void product.createSession(workspace)}
                    >
                      New conversation
                    </button>
                  ) : (
                    workspaceSessions.map((session) => (
                      <button
                        type="button"
                        key={session.sessionId}
                        className={
                          product.selectedSession?.sessionId === session.sessionId && !useLauncher
                            ? "session-link session-link-active"
                            : "session-link"
                        }
                        aria-current={
                          product.selectedSession?.sessionId === session.sessionId && !useLauncher
                            ? "page"
                            : undefined
                        }
                        onClick={() => {
                          setShowStartCenter(false);
                          void product.selectSession(session);
                        }}
                      >
                        <span>{sessionName(session)}</span>
                        <small>{shortTime(session.updatedAt)}</small>
                      </button>
                    ))
                  )}
                </section>
              );
            })}

            {(sessionsByWorkspace.get("history-only") ?? []).length > 0 ? (
              <section>
                <div className="workspace-nav-heading workspace-nav-muted">
                  <span className="workspace-dot" aria-hidden="true" />
                  <span>History only</span>
                </div>
                {(sessionsByWorkspace.get("history-only") ?? []).map((session) => (
                  <button
                    type="button"
                    key={session.sessionId}
                    className={
                      product.selectedSession?.sessionId === session.sessionId && !useLauncher
                        ? "session-link session-link-active"
                        : "session-link"
                    }
                    aria-current={
                      product.selectedSession?.sessionId === session.sessionId && !useLauncher
                        ? "page"
                        : undefined
                    }
                    onClick={() => {
                      setShowStartCenter(false);
                      void product.selectSession(session);
                    }}
                  >
                    <span>{sessionName(session)}</span>
                    <small>{shortTime(session.updatedAt)}</small>
                  </button>
                ))}
              </section>
            ) : null}
          </nav>

          {product.hasMoreSessions ? (
            <button
              type="button"
              className="load-more-sessions"
              disabled={product.sessionsLoadingMore}
              onClick={() => void product.loadMoreSessions()}
            >
              {product.sessionsLoadingMore ? "Loading…" : "Load older conversations"}
            </button>
          ) : null}

          <div className="sidebar-footnote">
            <span className="connection-dot" aria-hidden="true" />
            Runtime connected
          </div>
        </aside>
      ) : null}

      <section className="product-content">
        {product.activeOperation !== undefined ? (
          <div className="product-operation-status" role="status" aria-live="polite">
            <span aria-hidden="true" />
            {operationStatus(product.activeOperation)}
          </div>
        ) : null}
        <div
          className="product-content-base"
          aria-hidden={modalOpen ? "true" : undefined}
          inert={modalOpen ? true : undefined}
        >
          {product.notice !== undefined || product.recoveryPending ? (
            <div className="product-notice" role="status">
              <span>
                {product.notice ??
                  "A local change has an unresolved outcome. Recover it without repeating the action."}
              </span>
              {product.recoveryPending ? (
                <button
                  type="button"
                  disabled={product.busy}
                  onClick={() => void product.retryPendingOperations()}
                >
                  Retry recovery
                </button>
              ) : null}
            </div>
          ) : null}

          {useLauncher ? (
            <StartCenter
              busy={product.busy}
              workspaces={product.workspaces}
              onCreateWorkspace={async (intent, label) => {
                await product.createWorkspace(intent, label);
                setShowStartCenter(false);
              }}
              onCreateSession={async (workspace) => {
                await product.createSession(workspace);
                setShowStartCenter(false);
              }}
            />
          ) : (
            <div className="conversation-layout">
              <header className="conversation-header">
                <div>
                  <p>
                    {workspaceAvailable ? workspaceName(product.selectedWorkspace) : "History only"}
                  </p>
                  <h1>
                    {product.selectedSession === undefined
                      ? "Untitled conversation"
                      : sessionName(product.selectedSession)}
                  </h1>
                </div>
                <div className="conversation-header-actions">
                  {!conversationWindow ? (
                    <label
                      className={product.profileReady ? "profile-picker" : "profile-picker warning"}
                    >
                      <span>New runs</span>
                      <select
                        aria-label="Default profile for new runs"
                        value={product.modelSelection?.selectedProfile ?? ""}
                        disabled={
                          product.busy ||
                          product.profileSelectionRecoveryPending ||
                          product.profiles.length === 0
                        }
                        onChange={(event) => void product.selectProfile(event.currentTarget.value)}
                      >
                        {product.profiles.map((profile) => (
                          <option key={profile.name} value={profile.name}>
                            {profile.label ?? profile.name}
                          </option>
                        ))}
                      </select>
                    </label>
                  ) : null}
                  {waiting ? (
                    <button type="button" onClick={() => setShowInteractionInbox(true)}>
                      Review request
                    </button>
                  ) : null}
                  {!conversationWindow && product.selectedSession !== undefined ? (
                    <button
                      type="button"
                      onClick={() =>
                        void openConversationWindow(product.selectedSession?.sessionId ?? "")
                      }
                    >
                      Open in new window
                    </button>
                  ) : null}
                  <span className={active === undefined ? "run-chip" : "run-chip run-chip-active"}>
                    {!sessionReady
                      ? product.sessionLoadState.kind
                      : active !== undefined && !product.activeRunControlKnown
                        ? "checking control"
                        : active !== undefined && !product.activeRunControllable
                          ? "active elsewhere"
                          : (active?.status ?? "ready")}
                  </span>
                </div>
              </header>

              <div
                ref={conversationScrollRef}
                className="conversation-scroll"
                role="log"
                aria-label="Conversation messages"
                aria-live="polite"
                aria-relevant="additions text"
                onScroll={() => {
                  const scroll = conversationScrollRef.current;
                  if (scroll === null) return;
                  const nearLatest =
                    scroll.scrollHeight - scroll.scrollTop - scroll.clientHeight <= 72;
                  followLatest.current = nearLatest;
                  setShowJumpLatest(!nearLatest);
                }}
              >
                {product.sessionLoadState.kind === "loading" ? (
                  <div className="conversation-empty" role="status">
                    <span aria-hidden="true">S</span>
                    <h2>Opening conversation</h2>
                    <p>
                      Loading durable runs and checking which active work can be controlled here.
                    </p>
                  </div>
                ) : product.sessionLoadState.kind === "error" ? (
                  <div className="conversation-empty" role="alert">
                    <span aria-hidden="true">S</span>
                    <h2>Conversation could not be opened</h2>
                    <p>
                      The existing local history remains unchanged. Try loading this conversation
                      again.
                    </p>
                    {product.selectedSession !== undefined ? (
                      <button
                        type="button"
                        onClick={() => {
                          const selected = product.selectedSession;
                          if (selected !== undefined) void product.selectSession(selected);
                        }}
                      >
                        Try again
                      </button>
                    ) : null}
                  </div>
                ) : product.runs.length === 0 ? (
                  <div className="conversation-empty">
                    <span aria-hidden="true">S</span>
                    <h2>What are we working on?</h2>
                    <p>
                      Ask Starweaver to inspect, explain, or change this workspace. Local filesystem
                      actions remain constrained by the registered workspace authority.
                    </p>
                  </div>
                ) : (
                  <div className="message-list">
                    {product.hasMoreRuns ? (
                      <button
                        type="button"
                        className="load-earlier-runs"
                        disabled={product.runsLoadingMore}
                        onClick={() => void product.loadMoreRuns()}
                      >
                        {product.runsLoadingMore ? "Loading earlier runs…" : "Load earlier runs"}
                      </button>
                    ) : null}
                    {product.runs.map((run) => (
                      <article className="message-pair" key={run.runId}>
                        {run.inputPreview ? (
                          <div className="message message-user">
                            <span>You</span>
                            <p>{run.inputPreview}</p>
                          </div>
                        ) : null}
                        <div className="message message-assistant">
                          <div className="assistant-label">
                            <span>Starweaver</span>
                            <small>{run.status}</small>
                          </div>
                          {(product.runTranscriptText?.[run.runId] ?? run.outputPreview) ? (
                            <p>{product.runTranscriptText?.[run.runId] ?? run.outputPreview}</p>
                          ) : (
                            <div className="response-pending">
                              <i />
                              <i />
                              <i />
                              <span>
                                {run.status === "waiting"
                                  ? "Waiting for your response"
                                  : run.status === "failed"
                                    ? "This run did not produce a response"
                                    : "Working"}
                              </span>
                            </div>
                          )}
                        </div>
                      </article>
                    ))}
                  </div>
                )}
              </div>

              {showJumpLatest ? (
                <button
                  type="button"
                  className="jump-latest"
                  onClick={() => {
                    const scroll = conversationScrollRef.current;
                    if (scroll === null) return;
                    followLatest.current = true;
                    setShowJumpLatest(false);
                    scrollToLatest(scroll, "smooth");
                  }}
                >
                  Jump to latest
                </button>
              ) : null}

              <form className="composer" onSubmit={(event) => void submit(event)}>
                {!sessionReady ? (
                  <p className="composer-disabled-copy">
                    {product.sessionLoadState.kind === "error"
                      ? "Reload this conversation before starting or controlling a run."
                      : "Run controls will be available after this conversation finishes loading."}
                  </p>
                ) : !workspaceAvailable ? (
                  <p className="composer-disabled-copy">
                    This conversation is available as history. Reopen its workspace to continue.
                  </p>
                ) : active !== undefined && !product.activeRunControllable ? (
                  <p className="composer-disabled-copy">
                    {product.activeRunControlKnown
                      ? "This run is active in another process. Live evidence remains readable here, but control stays with its owner."
                      : "Run control availability is being checked. Live evidence remains readable while control stays disabled."}
                  </p>
                ) : null}
                <textarea
                  aria-label={active === undefined ? "Message Starweaver" : "Steer active run"}
                  value={composer}
                  rows={1}
                  maxLength={16384}
                  disabled={
                    product.busy ||
                    !sessionReady ||
                    !workspaceAvailable ||
                    waiting ||
                    (active !== undefined && !product.activeRunControllable)
                  }
                  readOnly={product.promptRecoveryPending ?? false}
                  placeholder={
                    !sessionReady
                      ? product.sessionLoadState.kind === "error"
                        ? "Reload this conversation to continue"
                        : "Opening this conversation"
                      : waiting
                        ? "This run is waiting for an interaction"
                        : active !== undefined && !product.activeRunControllable
                          ? product.activeRunControlKnown
                            ? "This active run is controlled by another process"
                            : "Checking whether this run can be controlled here"
                          : product.promptRecoveryPending
                            ? "Retry the unresolved prompt outcome"
                            : active === undefined
                              ? "Ask Starweaver about this workspace…"
                              : "Send a direction to the active run…"
                  }
                  onChange={(event) => setComposer(event.currentTarget.value)}
                  onKeyDown={composerKeyDown}
                />
                <div className="composer-actions">
                  <span>
                    Enter to send · Shift Enter for a new line ·{" "}
                    {Array.from(composer).length.toLocaleString()}/16,384
                  </span>
                  <div>
                    {active !== undefined && product.activeRunControllable ? (
                      <button
                        type="button"
                        className="stop-action"
                        disabled={product.busy}
                        onClick={() => void product.interruptRun()}
                      >
                        Stop
                      </button>
                    ) : null}
                    <button
                      type="submit"
                      className="send-action"
                      disabled={
                        product.busy ||
                        !sessionReady ||
                        !workspaceAvailable ||
                        waiting ||
                        (active !== undefined && !product.activeRunControllable) ||
                        composer.trim().length === 0
                      }
                    >
                      {product.promptRecoveryPending
                        ? "Retry"
                        : active === undefined
                          ? "Send"
                          : "Steer"}
                    </button>
                  </div>
                </div>
              </form>
            </div>
          )}
        </div>
        {showInteractionInbox ? (
          <InteractionInbox product={product} onClose={() => setShowInteractionInbox(false)} />
        ) : null}
        {showSettings ? (
          <SettingsPanel
            product={product}
            desktopPreferences={desktopPreferences}
            onClose={() => setShowSettings(false)}
          />
        ) : null}
      </section>
    </main>
  );
}
