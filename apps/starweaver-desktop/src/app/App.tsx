import { useEffect, useState } from "react";
import { getDesktopWindowRoute, retryManagedRuntime } from "../bridge/desktop";
import type { DesktopStatus, DesktopWindowRoute, RuntimeStateName } from "../bridge/types";
import "./styles.css";
import { useDesktopStatus } from "./useDesktopStatus";
import { WorkspaceProduct } from "./WorkspaceProduct";
import { useDesktopPreferences } from "./useDesktopPreferences";

type RuntimeCopy = {
  eyebrow: string;
  title: string;
  body: string;
};

const RUNTIME_COPY: Record<RuntimeStateName, RuntimeCopy> = {
  unconfigured: {
    eyebrow: "Local runtime",
    title: "Preparing your workspace",
    body: "Starweaver is locating the verified local runtime that powers your sessions.",
  },
  starting: {
    eyebrow: "Starting",
    title: "Waking up Starweaver",
    body: "The local runtime is being verified and started. This usually takes only a moment.",
  },
  handshaking: {
    eyebrow: "Connecting",
    title: "Establishing a secure connection",
    body: "Desktop and the local runtime are confirming their protocol and storage compatibility.",
  },
  ready: {
    eyebrow: "Local workspace",
    title: "Starweaver is ready",
    body: "Your local runtime is connected to the shared Starweaver history. You can begin without moving or importing your existing sessions.",
  },
  draining: {
    eyebrow: "Finishing work",
    title: "Safely stopping the runtime",
    body: "Starweaver is preserving the latest session state before the local runtime stops.",
  },
  recovering: {
    eyebrow: "Recovering",
    title: "Restoring your connection",
    body: "The local runtime was interrupted. Starweaver is replaying durable state before reconnecting.",
  },
  stopped: {
    eyebrow: "Runtime stopped",
    title: "Your sessions are safe",
    body: "The local runtime is not running. Start it again when you are ready to continue.",
  },
  incompatible: {
    eyebrow: "Update required",
    title: "This runtime cannot be used",
    body: "Desktop rejected the bundled runtime because its protocol or storage contract does not match this build.",
  },
  failed: {
    eyebrow: "Could not start",
    title: "The local runtime needs attention",
    body: "Starweaver could not establish a usable local connection. Review the runtime status, then try again.",
  },
};

const RETRYABLE_STATES = new Set<RuntimeStateName>(["unconfigured", "stopped", "failed"]);

function RuntimeMark({ state }: { state: RuntimeStateName }) {
  const active = state === "starting" || state === "handshaking" || state === "recovering";
  return (
    <div className={`runtime-mark${active ? " runtime-mark-active" : ""}`} aria-hidden="true">
      <span />
      <span />
      <span />
    </div>
  );
}

function RuntimePanel({ status }: { status: DesktopStatus }) {
  const [retrying, setRetrying] = useState(false);
  const copy = RUNTIME_COPY[status.runtime.state];
  const canRetry =
    RETRYABLE_STATES.has(status.runtime.state) && status.runtimeIssue?.retryable !== false;

  const retry = async () => {
    setRetrying(true);
    try {
      await retryManagedRuntime();
    } catch {
      // The next safe status projection contains the actionable category.
    } finally {
      setRetrying(false);
    }
  };

  return (
    <main className="runtime-screen">
      <section className="runtime-card" aria-labelledby="runtime-title">
        <RuntimeMark state={status.runtime.state} />
        <p className="eyebrow">{copy.eyebrow}</p>
        <h1 id="runtime-title">{copy.title}</h1>
        <p className="runtime-copy">{copy.body}</p>

        {status.runtimeIssue ? (
          <div className="runtime-issue" role="alert">
            <span>Runtime status</span>
            <p>{status.runtimeIssue.message}</p>
            <small>
              Safe diagnostic: {status.runtimeIssue.code.replaceAll("_", " ")}
              {status.runtimeIssue.reconciliationRequired ? " · local reconciliation required" : ""}
            </small>
          </div>
        ) : null}

        {canRetry ? (
          <button
            className="primary-action"
            type="button"
            onClick={() => void retry()}
            disabled={retrying}
          >
            {retrying ? "Starting…" : "Try again"}
          </button>
        ) : null}

        <dl className="runtime-details" aria-label="Desktop connection details">
          <div>
            <dt>Desktop</dt>
            <dd>v{status.appVersion}</dd>
          </div>
          <div>
            <dt>Runtime</dt>
            <dd>{status.runtime.state.replaceAll("_", " ")}</dd>
          </div>
          <div>
            <dt>History</dt>
            <dd>Shared local</dd>
          </div>
          <div>
            <dt>System</dt>
            <dd>
              {status.platform} · {status.architecture}
            </dd>
          </div>
        </dl>
      </section>
    </main>
  );
}

export default function App() {
  const [windowRoute, setWindowRoute] = useState<DesktopWindowRoute>();
  const [windowRouteFailed, setWindowRouteFailed] = useState(false);
  const state = useDesktopStatus(windowRoute !== undefined, windowRoute?.kind === "main");
  const desktopPreferences = useDesktopPreferences();

  useEffect(() => {
    let active = true;
    void getDesktopWindowRoute().then(
      (route) => {
        if (active) setWindowRoute(route);
      },
      () => {
        if (active) setWindowRouteFailed(true);
      },
    );
    return () => {
      active = false;
    };
  }, []);
  const preferences = desktopPreferences.snapshot?.preferences;

  return (
    <div
      className="desktop-root"
      data-theme={preferences?.theme ?? "system"}
      data-density={preferences?.density ?? "comfortable"}
    >
      <header className="titlebar">
        <div className="brand-mark" aria-hidden="true">
          <img src="/favicon.png" alt="" />
        </div>
        <span>
          {windowRoute?.kind === "conversation" ? "Starweaver Conversation" : "Starweaver"}
        </span>
      </header>

      {!windowRouteFailed && (state.kind === "loading" || windowRoute === undefined) ? (
        <main className="runtime-screen" aria-live="polite">
          <section className="runtime-card runtime-card-loading">
            <RuntimeMark state="starting" />
            <p className="eyebrow">Local workspace</p>
            <h1>Opening Starweaver</h1>
            <p className="runtime-copy">Checking the Desktop service and your local runtime.</p>
          </section>
        </main>
      ) : null}

      {state.kind === "error" || windowRouteFailed ? (
        <main className="runtime-screen">
          <section className="runtime-card" role="alert">
            <RuntimeMark state="failed" />
            <p className="eyebrow">Desktop service</p>
            <h1>Starweaver could not open</h1>
            <p className="runtime-copy">
              The privileged Desktop service is unavailable. Restart Starweaver to reconnect without
              changing your sessions.
            </p>
          </section>
        </main>
      ) : null}

      {state.kind === "ready" &&
      state.status.runtime.state === "ready" &&
      windowRoute !== undefined ? (
        <WorkspaceProduct desktopPreferences={desktopPreferences} windowRoute={windowRoute} />
      ) : null}

      {state.kind === "ready" &&
      state.status.runtime.state !== "ready" &&
      windowRoute !== undefined &&
      !windowRouteFailed ? (
        <RuntimePanel status={state.status} />
      ) : null}

      <footer className="privacy-note">
        Local by design · Credentials and session data stay outside the interface
      </footer>
    </div>
  );
}
