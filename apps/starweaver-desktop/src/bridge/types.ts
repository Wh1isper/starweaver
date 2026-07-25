export type RuntimeStateName =
  | "unconfigured"
  | "starting"
  | "handshaking"
  | "ready"
  | "draining"
  | "recovering"
  | "stopped"
  | "incompatible"
  | "failed";

export type RuntimeState = {
  state: RuntimeStateName;
  generation: number;
  diagnosticsAvailable: boolean;
};

export type RuntimeIssue = {
  code:
    | "not_ready"
    | "invalid_configuration"
    | "incompatible"
    | "transport"
    | "remote"
    | "internal";
  message: string;
  remoteCode?: number;
  remoteKind?: string;
  retryable?: boolean;
  reconciliationRequired?: boolean;
  resourceKind?: string;
  operationAcknowledgementToken?: string;
};

export type DesktopStatus = {
  appVersion: string;
  platform: "linux" | "macos" | "windows" | "unknown";
  architecture: string;
  launchGeneration: number;
  singleInstance: true;
  runtime: RuntimeState;
  runtimeIssue?: RuntimeIssue;
};

export type DesktopActivation = {
  kind: "secondary_launch";
  generation: number;
};

export type DesktopWindowRoute =
  | { readonly kind: "main" }
  | { readonly kind: "conversation"; readonly sessionId: string };

export type DesktopConversationWindow = {
  readonly label: string;
  readonly reused: boolean;
  readonly sessionId: string;
};

export type DesktopTheme = "system" | "light" | "dark";
export type DesktopDensity = "comfortable" | "compact";
export type WindowCloseBehavior = "keep_running" | "quit";

export type DesktopPreferences = {
  theme: DesktopTheme;
  density: DesktopDensity;
  windowCloseBehavior: WindowCloseBehavior;
};

export type DesktopPreferencesSnapshot = {
  schemaVersion: 1;
  revision: string;
  preferences: DesktopPreferences;
  loadIssue?: string;
};

export type DesktopPreferencesUpdate = {
  expectedRevision: string;
  mutationId: string;
  preferences: DesktopPreferences;
};

export type DesktopPreferencesError = {
  code: "not_ready" | "invalid_request" | "conflict" | "storage";
  message: string;
  currentRevision?: string;
};
