import type { ModelSelection, ProfileDetail, ProfileSummary } from "./workspaceProductTypes";

export function deriveProfileReadiness(
  profiles: readonly ProfileSummary[],
  modelSelection: ModelSelection | undefined,
  profileDetail: ProfileDetail | undefined,
): {
  readonly selectedProfile: ProfileSummary | undefined;
  readonly profileReady: boolean;
  readonly profileReadinessIssue: string | undefined;
} {
  const selectedProfile = profiles.find(
    (profile) => profile.name === modelSelection?.selectedProfile,
  );
  const profileReady =
    selectedProfile !== undefined &&
    modelSelection?.modelId === selectedProfile.modelId &&
    profileDetail?.name === selectedProfile.name &&
    profileDetail.modelId === selectedProfile.modelId;
  const profileReadinessIssue =
    modelSelection === undefined
      ? "Profile catalog is unavailable."
      : selectedProfile === undefined
        ? "The saved default profile is no longer in the active runtime catalog."
        : modelSelection.modelId !== selectedProfile.modelId
          ? "The saved profile selection is stale for the active runtime catalog."
          : profileDetail === undefined
            ? "The selected profile could not be materialized."
            : profileDetail.modelId !== selectedProfile.modelId
              ? "The selected profile changed while it was being materialized."
              : undefined;
  return { profileReadinessIssue, profileReady, selectedProfile };
}
