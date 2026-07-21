import { useEffect, useState } from "react";
import { getDesktopStatus, onDesktopActivation } from "../bridge/desktop";
import type { DesktopStatus } from "../bridge/types";

type StatusState =
  | { kind: "loading" }
  | { kind: "ready"; status: DesktopStatus }
  | { kind: "error" };

export function useDesktopStatus(): StatusState {
  const [state, setState] = useState<StatusState>({ kind: "loading" });

  useEffect(() => {
    let active = true;
    let latestRequest = 0;
    let unlisten: (() => void) | undefined;

    const refresh = async () => {
      const request = ++latestRequest;
      try {
        const status = await getDesktopStatus();
        if (active && request === latestRequest) {
          setState({ kind: "ready", status });
        }
      } catch {
        if (active && request === latestRequest) {
          setState({ kind: "error" });
        }
      }
    };

    const initialize = async () => {
      try {
        const stopListening = await onDesktopActivation(() => {
          void refresh();
        });
        if (!active) {
          stopListening();
          return;
        }
        unlisten = stopListening;
        await refresh();
      } catch {
        if (active) setState({ kind: "error" });
      }
    };

    void initialize();
    return () => {
      active = false;
      unlisten?.();
    };
  }, []);

  return state;
}
