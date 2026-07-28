import { useEffect, useState } from "react";
import {
  checkDesktopUpdate,
  checkRuntimeUpdate,
  getDesktopUpdateStatus,
  getRuntimeUpdateStatus,
  installDesktopUpdate,
  installRuntimeUpdate,
  rollbackRuntimeUpdate,
} from "../bridge/desktop";
import type { DesktopUpdateSnapshot, RuntimeUpdateSnapshot } from "../bridge/types";

function safeErrorMessage(error: unknown): string {
  if (
    typeof error === "object" &&
    error !== null &&
    "message" in error &&
    typeof error.message === "string"
  ) {
    return error.message;
  }
  return "The update operation could not be completed.";
}

function sizeLabel(bytes: number): string {
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

export function UpdateSettings() {
  const [desktop, setDesktop] = useState<DesktopUpdateSnapshot>();
  const [runtime, setRuntime] = useState<RuntimeUpdateSnapshot>();
  const [busy, setBusy] = useState<"desktop" | "runtime">();
  const [issue, setIssue] = useState<string>();

  useEffect(() => {
    let active = true;
    void Promise.all([getDesktopUpdateStatus(), getRuntimeUpdateStatus()]).then(
      ([desktopStatus, runtimeStatus]) => {
        if (!active) return;
        setDesktop(desktopStatus);
        setRuntime(runtimeStatus);
      },
      (error: unknown) => {
        if (active) setIssue(safeErrorMessage(error));
      },
    );
    return () => {
      active = false;
    };
  }, []);

  const checkDesktop = async () => {
    setBusy("desktop");
    setIssue(undefined);
    try {
      setDesktop(await checkDesktopUpdate());
    } catch (error: unknown) {
      setIssue(safeErrorMessage(error));
    } finally {
      setBusy(undefined);
    }
  };

  const applyDesktop = async () => {
    if (desktop?.candidate === undefined) return;
    setBusy("desktop");
    setIssue(undefined);
    try {
      await installDesktopUpdate(desktop.candidate.version);
    } catch (error: unknown) {
      setIssue(safeErrorMessage(error));
      setBusy(undefined);
    }
  };

  const checkRuntime = async () => {
    setBusy("runtime");
    setIssue(undefined);
    try {
      setRuntime(await checkRuntimeUpdate());
    } catch (error: unknown) {
      setIssue(safeErrorMessage(error));
    } finally {
      setBusy(undefined);
    }
  };

  const applyRuntime = async () => {
    if (runtime?.candidate === undefined) return;
    setBusy("runtime");
    setIssue(undefined);
    try {
      setRuntime(await installRuntimeUpdate(runtime.candidate.candidateId));
    } catch (error: unknown) {
      setIssue(safeErrorMessage(error));
    } finally {
      setBusy(undefined);
    }
  };

  const rollbackRuntime = async () => {
    setBusy("runtime");
    setIssue(undefined);
    try {
      setRuntime(await rollbackRuntimeUpdate());
    } catch (error: unknown) {
      setIssue(safeErrorMessage(error));
    } finally {
      setBusy(undefined);
    }
  };

  return (
    <div className="settings-section">
      <div className="settings-section-heading">
        <div>
          <h3>Product updates</h3>
          <p>
            Desktop and the RPC runtime use separate, fixed Starweaver release channels. Neither
            channel accepts a renderer-provided URL.
          </p>
        </div>
      </div>

      <div className="readiness-card">
        <span>Desktop application</span>
        <strong>{desktop === undefined ? "Loading…" : `v${desktop.currentVersion}`}</strong>
        {desktop?.candidate ? (
          <>
            <p>Version {desktop.candidate.version} is available.</p>
            {desktop.candidate.notes ? <small>{desktop.candidate.notes}</small> : null}
          </>
        ) : (
          <p>
            {desktop?.configured === false
              ? "This development build has no updater trust key."
              : "No newer checked Desktop release."}
          </p>
        )}
        <small>
          Tauri verifies update artifacts with the project key. Apple Developer ID and Windows
          Authenticode are not configured, so operating-system publisher warnings can still appear.
        </small>
        <div className="settings-inline-actions">
          <button
            type="button"
            disabled={busy !== undefined || desktop?.configured !== true}
            onClick={() => void checkDesktop()}
          >
            {busy === "desktop" ? "Checking…" : "Check Desktop update"}
          </button>
          {desktop?.candidate ? (
            <button type="button" disabled={busy !== undefined} onClick={() => void applyDesktop()}>
              Install and restart
            </button>
          ) : null}
        </div>
      </div>

      <div className="readiness-card">
        <span>RPC runtime</span>
        <strong>
          {runtime === undefined
            ? "Loading…"
            : runtime.activeVersion
              ? `Running v${runtime.activeVersion} (${runtime.activeSource ?? "unknown"}) · next v${runtime.selectedVersion} (${runtime.selectedSource})`
              : `Next start v${runtime.selectedVersion} (${runtime.selectedSource})`}
        </strong>
        {runtime?.candidate ? (
          <p>
            Version {runtime.candidate.version} for {runtime.candidate.target} is available (
            {sizeLabel(runtime.candidate.size)}).
          </p>
        ) : (
          <p>
            {runtime?.configured === false
              ? "This development build has no runtime update trust key."
              : "No newer compatible RPC release."}
          </p>
        )}
        <small>
          RPC updates must keep the exact host protocol and storage generation. They are downloaded
          to a private version directory, hash-checked, and probed before next-start activation.
        </small>
        <div className="settings-inline-actions">
          <button
            type="button"
            disabled={busy !== undefined || runtime?.configured !== true}
            onClick={() => void checkRuntime()}
          >
            {busy === "runtime" ? "Working…" : "Check RPC update"}
          </button>
          {runtime?.candidate ? (
            <button type="button" disabled={busy !== undefined} onClick={() => void applyRuntime()}>
              Verify and use after restart
            </button>
          ) : null}
          {runtime?.selectedSource === "managed" ? (
            <button
              type="button"
              disabled={busy !== undefined}
              onClick={() => void rollbackRuntime()}
            >
              Select previous or bundled runtime
            </button>
          ) : null}
        </div>
        {runtime?.restartRequired ? (
          <p className="settings-validation" role="status">
            Restart Starweaver to activate the selected RPC runtime.
          </p>
        ) : null}
      </div>

      {issue ? (
        <div className="settings-validation warning" role="alert">
          <strong>Update needs attention</strong>
          <p>{issue}</p>
        </div>
      ) : null}
    </div>
  );
}
