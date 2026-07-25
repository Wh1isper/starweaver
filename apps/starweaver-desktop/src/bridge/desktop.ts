import { Channel, invoke } from "@tauri-apps/api/core";
import {
  ACKNOWLEDGE_HOST_OPERATION_COMMAND,
  DesktopHostAcknowledgementError,
  DesktopHostExecutionError,
  EXECUTE_HOST_OPERATION_COMMAND,
} from "../generated/host/client";
import type {
  DesktopHostInvocation,
  DesktopHostOperationAcknowledgementToken,
  WorkspaceRegisterResult,
} from "../generated/host/types";
import { parseDesktopHostOperationDelivery } from "../generated/host/validators";
import type {
  DesktopActivation,
  DesktopConversationWindow,
  DesktopPreferencesSnapshot,
  DesktopPreferencesUpdate,
  DesktopStatus,
  DesktopWindowRoute,
} from "./types";

const GET_DESKTOP_STATUS_COMMAND = "get_desktop_status";
const RETRY_MANAGED_RUNTIME_COMMAND = "retry_managed_runtime";
const GET_DESKTOP_PREFERENCES_COMMAND = "get_desktop_preferences";
const UPDATE_DESKTOP_PREFERENCES_COMMAND = "update_desktop_preferences";
const RELOAD_DESKTOP_PREFERENCES_COMMAND = "reload_desktop_preferences";
const SUBSCRIBE_DESKTOP_ACTIVATION_COMMAND = "subscribe_desktop_activation";
const UNSUBSCRIBE_DESKTOP_ACTIVATION_COMMAND = "unsubscribe_desktop_activation";
const GET_DESKTOP_WINDOW_ROUTE_COMMAND = "get_desktop_window_route";
const OPEN_CONVERSATION_WINDOW_COMMAND = "open_conversation_window";

export type DesktopWorkspaceIntent =
  | { readonly kind: "open_existing" }
  | { readonly kind: "create_empty"; readonly name: string }
  | { readonly kind: "managed" };

export function getDesktopStatus(): Promise<DesktopStatus> {
  return invoke<DesktopStatus>(GET_DESKTOP_STATUS_COMMAND);
}

export function retryManagedRuntime(): Promise<void> {
  return invoke<void>(RETRY_MANAGED_RUNTIME_COMMAND);
}

export function getDesktopPreferences(): Promise<DesktopPreferencesSnapshot> {
  return invoke<DesktopPreferencesSnapshot>(GET_DESKTOP_PREFERENCES_COMMAND);
}

export function updateDesktopPreferences(
  update: DesktopPreferencesUpdate,
): Promise<DesktopPreferencesSnapshot> {
  return invoke<DesktopPreferencesSnapshot>(UPDATE_DESKTOP_PREFERENCES_COMMAND, { update });
}

export function reloadDesktopPreferences(): Promise<DesktopPreferencesSnapshot> {
  return invoke<DesktopPreferencesSnapshot>(RELOAD_DESKTOP_PREFERENCES_COMMAND);
}

export function getDesktopWindowRoute(): Promise<DesktopWindowRoute> {
  return invoke<DesktopWindowRoute>(GET_DESKTOP_WINDOW_ROUTE_COMMAND);
}

export function openConversationWindow(sessionId: string): Promise<DesktopConversationWindow> {
  return invoke<DesktopConversationWindow>(OPEN_CONVERSATION_WINDOW_COMMAND, { sessionId });
}

export async function executeDesktopWorkspaceRegistration(
  invocation: DesktopHostInvocation<"workspace.register">,
  workspaceIntent?: DesktopWorkspaceIntent,
): Promise<WorkspaceRegisterResult> {
  let delivery: {
    readonly acknowledgementToken?: DesktopHostOperationAcknowledgementToken;
    readonly result: WorkspaceRegisterResult;
  };
  try {
    const value: unknown = await invoke(EXECUTE_HOST_OPERATION_COMMAND, {
      invocation,
      workspaceIntent,
    });
    delivery = parseDesktopHostOperationDelivery("workspace.register", value);
  } catch (error: unknown) {
    const token =
      typeof error === "object" && error !== null && "operationAcknowledgementToken" in error
        ? error.operationAcknowledgementToken
        : undefined;
    if (typeof token === "string" && token.startsWith("desktop-operation-ack-v1-")) {
      const acknowledgementToken = token as DesktopHostOperationAcknowledgementToken;
      try {
        await invoke(ACKNOWLEDGE_HOST_OPERATION_COMMAND, { acknowledgementToken });
      } catch (acknowledgementError: unknown) {
        throw new DesktopHostAcknowledgementError(
          invocation,
          acknowledgementToken,
          undefined,
          error,
          acknowledgementError,
        );
      }
      throw error;
    }
    throw new DesktopHostExecutionError(invocation, error);
  }
  if (delivery.acknowledgementToken !== undefined) {
    try {
      await invoke(ACKNOWLEDGE_HOST_OPERATION_COMMAND, {
        acknowledgementToken: delivery.acknowledgementToken,
      });
    } catch (error: unknown) {
      throw new DesktopHostAcknowledgementError(
        invocation,
        delivery.acknowledgementToken,
        delivery.result,
        undefined,
        error,
      );
    }
  }
  return delivery.result;
}

export async function onDesktopActivation(
  handler: (activation: DesktopActivation) => void,
): Promise<() => void> {
  let active = true;
  const channel = new Channel<DesktopActivation>((activation) => {
    if (active) handler(activation);
  });
  const subscriptionToken = await invoke<number>(SUBSCRIBE_DESKTOP_ACTIVATION_COMMAND, {
    onActivation: channel,
  });
  return () => {
    active = false;
    void invoke<void>(UNSUBSCRIBE_DESKTOP_ACTIVATION_COMMAND, { subscriptionToken }).catch(
      () => undefined,
    );
  };
}
