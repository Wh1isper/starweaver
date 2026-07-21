export type RuntimeState = {
  state: "unavailable";
  reason: "not_configured";
};

export type DesktopStatus = {
  appVersion: string;
  platform: "linux" | "macos" | "windows" | "unknown";
  architecture: string;
  launchGeneration: number;
  singleInstance: true;
  runtime: RuntimeState;
};

export type DesktopActivation = {
  kind: "secondary_launch";
  generation: number;
};
