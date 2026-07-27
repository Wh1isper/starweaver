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

export type RuntimeSelectionSource = "bundled" | "managed";

export type RuntimeUpdateCandidate = {
  candidateId: string;
  version: string;
  buildRevision: string;
  target: string;
  size: number;
};

export type RuntimeUpdateSnapshot = {
  configured: boolean;
  activeVersion?: string;
  activeSource?: RuntimeSelectionSource;
  selectedVersion: string;
  selectedSource: RuntimeSelectionSource;
  candidate?: RuntimeUpdateCandidate;
  restartRequired: boolean;
};

export type RuntimeUpdateError = {
  code: "unavailable" | "network" | "verification" | "storage" | "stale_candidate" | "probe";
  message: string;
};

export type DesktopUpdateCandidate = {
  version: string;
  notes?: string;
  publishedAt?: string;
  platformPublisherSigned: false;
};

export type DesktopUpdateSnapshot = {
  currentVersion: string;
  configured: boolean;
  candidate?: DesktopUpdateCandidate;
};

export type DesktopUpdateError = {
  code:
    | "unavailable"
    | "network"
    | "verification"
    | "stale_candidate"
    | "installation"
    | "cancelled";
  message: string;
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
