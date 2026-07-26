import { useEffect, useState } from "react";
import { getDesktopStatus, onDesktopActivation } from "../bridge/desktop";
import type { DesktopStatus } from "../bridge/types";

type StatusState =
  | { kind: "loading" }
  | { kind: "ready"; status: DesktopStatus }
  | { kind: "error" };

const ACTIVE_REFRESH_MS = 750;
const IDLE_REFRESH_MS = 5_000;
const ERROR_REFRESH_MS = 2_000;

export function useDesktopStatus(enabled = true, listenForActivation = true): StatusState {
  const [state, setState] = useState<StatusState>({ kind: "loading" });

  useEffect(() => {
    if (!enabled) {
      setState({ kind: "loading" });
      return;
    }
    let active = true;
    let latestRequest = 0;
    let refreshTimer: number | undefined;
    let unlisten: (() => void) | undefined;

    const scheduleRefresh = (delay: number) => {
      window.clearTimeout(refreshTimer);
      if (document.visibilityState === "hidden") return;
      refreshTimer = window.setTimeout(() => void refresh(), delay);
    };

    const refresh = async () => {
      const request = ++latestRequest;
      try {
        const status = await getDesktopStatus();
        if (!active || request !== latestRequest) return;
        setState({ kind: "ready", status });
        scheduleRefresh(status.runtime.state === "ready" ? IDLE_REFRESH_MS : ACTIVE_REFRESH_MS);
      } catch {
        if (!active || request !== latestRequest) return;
        setState({ kind: "error" });
        scheduleRefresh(ERROR_REFRESH_MS);
      }
    };

    const initialize = async () => {
      try {
        if (listenForActivation) {
          const stopListening = await onDesktopActivation(() => {
            void refresh();
          });
          if (!active) {
            stopListening();
            return;
          }
          unlisten = stopListening;
        }
        await refresh();
      } catch {
        if (active) {
          setState({ kind: "error" });
          scheduleRefresh(ERROR_REFRESH_MS);
        }
      }
    };

    const onVisibilityChange = () => {
      window.clearTimeout(refreshTimer);
      if (document.visibilityState !== "hidden") void refresh();
    };
    document.addEventListener("visibilitychange", onVisibilityChange);
    void initialize();
    return () => {
      active = false;
      window.clearTimeout(refreshTimer);
      document.removeEventListener("visibilitychange", onVisibilityChange);
      unlisten?.();
    };
  }, [enabled, listenForActivation]);

  return state;
}
