import { Channel, invoke } from "@tauri-apps/api/core";
import type { DesktopActivation, DesktopStatus } from "./types";

const GET_DESKTOP_STATUS_COMMAND = "get_desktop_status";
const SUBSCRIBE_DESKTOP_ACTIVATION_COMMAND = "subscribe_desktop_activation";
const UNSUBSCRIBE_DESKTOP_ACTIVATION_COMMAND = "unsubscribe_desktop_activation";

export function getDesktopStatus(): Promise<DesktopStatus> {
  return invoke<DesktopStatus>(GET_DESKTOP_STATUS_COMMAND);
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
