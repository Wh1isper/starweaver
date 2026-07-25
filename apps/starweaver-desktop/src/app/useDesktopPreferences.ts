import { useCallback, useEffect, useRef, useState } from "react";
import {
  getDesktopPreferences,
  reloadDesktopPreferences,
  updateDesktopPreferences,
} from "../bridge/desktop";
import type {
  DesktopPreferences,
  DesktopPreferencesSnapshot,
  DesktopPreferencesUpdate,
} from "../bridge/types";

export type DesktopPreferencesState = {
  readonly snapshot?: DesktopPreferencesSnapshot;
  readonly loading: boolean;
  readonly saving: boolean;
  readonly issue?: string;
  readonly recoveryPending: boolean;
  readonly save: (preferences: DesktopPreferences) => Promise<boolean>;
  readonly retryPending: () => Promise<boolean>;
  readonly reload: () => Promise<void>;
};

function preferenceMutationId(): string {
  return `desktop-preferences-${crypto.randomUUID()}`;
}

function publicIssue(error: unknown): string {
  if (typeof error === "object" && error !== null && "message" in error) {
    const message = error.message;
    if (typeof message === "string" && message.length > 0 && message.length <= 240) return message;
  }
  return "Desktop preferences could not be saved.";
}

export function useDesktopPreferences(): DesktopPreferencesState {
  const [snapshot, setSnapshot] = useState<DesktopPreferencesSnapshot>();
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [issue, setIssue] = useState<string>();
  const pendingRef = useRef<DesktopPreferencesUpdate | undefined>(undefined);
  const mountedRef = useRef(true);

  useEffect(() => {
    mountedRef.current = true;
    void getDesktopPreferences()
      .then((value) => {
        if (!mountedRef.current) return;
        setSnapshot(value);
        setIssue(value.loadIssue);
      })
      .catch((error: unknown) => {
        if (mountedRef.current) setIssue(publicIssue(error));
      })
      .finally(() => {
        if (mountedRef.current) setLoading(false);
      });
    return () => {
      mountedRef.current = false;
    };
  }, []);

  const execute = useCallback(async (update: DesktopPreferencesUpdate): Promise<boolean> => {
    setSaving(true);
    setIssue(undefined);
    try {
      const value = await updateDesktopPreferences(update);
      if (!mountedRef.current) return false;
      pendingRef.current = undefined;
      setSnapshot(value);
      setIssue(value.loadIssue);
      return true;
    } catch (error: unknown) {
      if (mountedRef.current) setIssue(publicIssue(error));
      return false;
    } finally {
      if (mountedRef.current) setSaving(false);
    }
  }, []);

  const save = useCallback(
    async (preferences: DesktopPreferences): Promise<boolean> => {
      if (snapshot === undefined || saving || pendingRef.current !== undefined) return false;
      const update: DesktopPreferencesUpdate = {
        expectedRevision: snapshot.revision,
        mutationId: preferenceMutationId(),
        preferences,
      };
      pendingRef.current = update;
      return execute(update);
    },
    [execute, saving, snapshot],
  );

  const retryPending = useCallback(async (): Promise<boolean> => {
    const pending = pendingRef.current;
    if (pending === undefined || saving) return false;
    return execute(pending);
  }, [execute, saving]);

  const reload = useCallback(async (): Promise<void> => {
    if (saving || pendingRef.current !== undefined) return;
    setLoading(true);
    setIssue(undefined);
    try {
      const value = await reloadDesktopPreferences();
      if (!mountedRef.current) return;
      pendingRef.current = undefined;
      setSnapshot(value);
      setIssue(value.loadIssue);
    } catch (error: unknown) {
      if (mountedRef.current) setIssue(publicIssue(error));
    } finally {
      if (mountedRef.current) setLoading(false);
    }
  }, [saving]);

  return {
    snapshot,
    loading,
    saving,
    issue,
    recoveryPending: pendingRef.current !== undefined,
    save,
    retryPending,
    reload,
  };
}
