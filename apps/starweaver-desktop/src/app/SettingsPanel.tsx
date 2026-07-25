import { useEffect, useMemo, useState } from "react";
import type { DesktopPreferences } from "../bridge/types";
import type { DesktopPreferencesState } from "./useDesktopPreferences";
import { useModalDialog } from "./useModalDialog";
import type { RuntimeConfigDocument, useWorkspaceProduct } from "./useWorkspaceProduct";

type Product = ReturnType<typeof useWorkspaceProduct>;
type SettingsSection = "appearance" | "profile" | "runtime" | "providers" | "source";

function lines(value: string): readonly string[] {
  return value
    .split("\n")
    .map((entry) => entry.trim())
    .filter((entry, index, entries) => entry.length > 0 && entries.indexOf(entry) === index);
}

function commaList(value: string): readonly string[] {
  return value
    .split(",")
    .map((entry) => entry.trim())
    .filter((entry, index, entries) => entry.length > 0 && entries.indexOf(entry) === index);
}

export function SettingsPanel({
  product,
  desktopPreferences,
  onClose,
}: {
  product: Product;
  desktopPreferences?: DesktopPreferencesState;
  onClose: () => void;
}) {
  const dialogRef = useModalDialog<HTMLElement>(onClose);
  const [section, setSection] = useState<SettingsSection>("appearance");
  const [draft, setDraft] = useState<RuntimeConfigDocument | undefined>(product.runtimeConfig);
  const [editingProfileName, setEditingProfileName] = useState<string | undefined>(
    product.runtimeConfig?.defaultProfile,
  );

  useEffect(() => {
    setDraft(product.runtimeConfig);
    setEditingProfileName(product.runtimeConfig?.defaultProfile);
  }, [product.runtimeConfig]);

  const dirty = useMemo(
    () =>
      draft !== undefined &&
      product.runtimeConfig !== undefined &&
      JSON.stringify(draft) !== JSON.stringify(product.runtimeConfig),
    [draft, product.runtimeConfig],
  );
  const selectedProfile =
    draft?.profiles.find((profile) => profile.name === editingProfileName) ?? draft?.profiles[0];
  const applicationPreferences = desktopPreferences?.snapshot?.preferences;

  const saveApplicationPreference = <Key extends keyof DesktopPreferences>(
    key: Key,
    value: DesktopPreferences[Key],
  ) => {
    if (applicationPreferences === undefined) return;
    void desktopPreferences?.save({ ...applicationPreferences, [key]: value });
  };

  const updateProfile = (
    name: string,
    update: (
      profile: RuntimeConfigDocument["profiles"][number],
    ) => RuntimeConfigDocument["profiles"][number],
  ) => {
    setDraft((current) =>
      current === undefined
        ? current
        : {
            ...current,
            profiles: current.profiles.map((profile) =>
              profile.name === name ? update(profile) : profile,
            ),
          },
    );
  };

  const updateProvider = (
    name: string,
    update: (
      provider: RuntimeConfigDocument["providers"][number],
    ) => RuntimeConfigDocument["providers"][number],
  ) => {
    setDraft((current) =>
      current === undefined
        ? current
        : {
            ...current,
            providers: current.providers.map((provider) =>
              provider.name === name ? update(provider) : provider,
            ),
          },
    );
  };

  return (
    <aside
      ref={dialogRef}
      className="settings-drawer"
      role="dialog"
      aria-modal="true"
      aria-labelledby="settings-title"
      tabIndex={-1}
    >
      <header className="settings-header">
        <div>
          <p className="eyebrow">Local Desktop settings</p>
          <h2 id="settings-title">Settings</h2>
        </div>
        <button
          type="button"
          aria-label="Close settings"
          data-dialog-initial-focus
          onClick={onClose}
        >
          Close
        </button>
      </header>

      <div className="settings-layout">
        <nav className="settings-navigation" aria-label="Settings sections">
          {(
            [
              ["appearance", "Appearance"],
              ["profile", "Default profile"],
              ["runtime", "Profile details"],
              ["providers", "Provider routes"],
              ["source", "Runtime source"],
            ] as const
          ).map(([value, label]) => (
            <button
              type="button"
              key={value}
              className={section === value ? "settings-nav-active" : undefined}
              aria-current={section === value ? "page" : undefined}
              onClick={() => setSection(value)}
            >
              {label}
            </button>
          ))}
        </nav>

        <section className="settings-content">
          {section === "appearance" ? (
            <div className="settings-section">
              <div className="settings-section-heading">
                <div>
                  <h3>Appearance and windows</h3>
                  <p>
                    These preferences belong to Desktop and never enter runtime configuration or
                    session evidence.
                  </p>
                </div>
              </div>
              {desktopPreferences === undefined ? (
                <p className="settings-empty">Desktop preferences are unavailable in this view.</p>
              ) : applicationPreferences === undefined ? (
                <p className="settings-loading">
                  {desktopPreferences.loading
                    ? "Loading private Desktop preferences…"
                    : "Private Desktop preferences could not be loaded."}
                </p>
              ) : (
                <>
                  <div className="settings-field-row">
                    <label className="settings-field">
                      <span>Theme</span>
                      <select
                        aria-label="Theme"
                        value={applicationPreferences.theme}
                        disabled={desktopPreferences.saving || desktopPreferences.recoveryPending}
                        onChange={(event) =>
                          saveApplicationPreference(
                            "theme",
                            event.currentTarget.value as DesktopPreferences["theme"],
                          )
                        }
                      >
                        <option value="system">Follow system</option>
                        <option value="light">Light</option>
                        <option value="dark">Dark</option>
                      </select>
                    </label>
                    <label className="settings-field">
                      <span>Density</span>
                      <select
                        aria-label="Density"
                        value={applicationPreferences.density}
                        disabled={desktopPreferences.saving || desktopPreferences.recoveryPending}
                        onChange={(event) =>
                          saveApplicationPreference(
                            "density",
                            event.currentTarget.value as DesktopPreferences["density"],
                          )
                        }
                      >
                        <option value="comfortable">Comfortable</option>
                        <option value="compact">Compact</option>
                      </select>
                    </label>
                  </div>
                  <label className="settings-field">
                    <span>When a window closes</span>
                    <select
                      aria-label="When a window closes"
                      value={applicationPreferences.windowCloseBehavior}
                      disabled={desktopPreferences.saving || desktopPreferences.recoveryPending}
                      onChange={(event) =>
                        saveApplicationPreference(
                          "windowCloseBehavior",
                          event.currentTarget.value as DesktopPreferences["windowCloseBehavior"],
                        )
                      }
                    >
                      <option value="keep_running">Keep Starweaver running</option>
                      <option value="quit">Quit Starweaver</option>
                    </select>
                    <small>
                      Keeping Starweaver running preserves active work. Launch the app again to
                      reopen a hidden window. Explicit application quit always performs coordinated
                      runtime shutdown.
                    </small>
                  </label>
                </>
              )}
              {desktopPreferences?.issue !== undefined ? (
                <div className="settings-validation warning" role="status">
                  <strong>Desktop preferences need attention</strong>
                  <p>{desktopPreferences.issue}</p>
                  <div className="settings-inline-actions">
                    {desktopPreferences.recoveryPending ? (
                      <button
                        type="button"
                        disabled={desktopPreferences.saving}
                        onClick={() => void desktopPreferences.retryPending()}
                      >
                        Retry exact save
                      </button>
                    ) : null}
                    <button
                      type="button"
                      disabled={desktopPreferences.saving || desktopPreferences.recoveryPending}
                      onClick={() => void desktopPreferences.reload()}
                    >
                      Reload private file
                    </button>
                  </div>
                </div>
              ) : null}
            </div>
          ) : null}

          {product.settingsLoading && draft === undefined && section !== "appearance" ? (
            <p className="settings-loading">Loading safe runtime settings…</p>
          ) : null}

          {section === "profile" ? (
            <div className="settings-section">
              <div className={product.profileReady ? "readiness-card" : "readiness-card warning"}>
                <span>
                  {product.profileReady ? "Ready for new runs" : "Profile needs attention"}
                </span>
                <strong>
                  {product.selectedProfile?.label ??
                    product.modelSelection?.selectedProfile ??
                    "No default profile"}
                </strong>
                <p>
                  {product.profileReadinessIssue ??
                    `${product.profileDetail?.modelId ?? "Unknown model"} · ${(product.profileDetail?.toolsets.length ?? 0).toString()} toolsets`}
                </p>
                <small>
                  Readiness confirms local catalog materialization only. Provider credentials and
                  network access remain host-local and are checked when a run starts.
                </small>
              </div>

              <label className="settings-field">
                <span>Default profile for new runs</span>
                <select
                  value={product.modelSelection?.selectedProfile ?? ""}
                  disabled={
                    product.busy ||
                    product.profileSelectionRecoveryPending ||
                    product.profiles.length === 0
                  }
                  onChange={(event) => void product.selectProfile(event.currentTarget.value)}
                >
                  {product.profiles.map((profile) => (
                    <option value={profile.name} key={profile.name}>
                      {profile.label ?? profile.name} · {profile.modelId}
                    </option>
                  ))}
                </select>
              </label>

              {product.profileDetail ? (
                <dl className="profile-facts">
                  <div>
                    <dt>Model</dt>
                    <dd>{product.profileDetail.modelId}</dd>
                  </div>
                  <div>
                    <dt>Tools</dt>
                    <dd>{product.profileDetail.toolsets.join(", ") || "None"}</dd>
                  </div>
                  <div>
                    <dt>MCP</dt>
                    <dd>{product.profileDetail.mcpServers.join(", ") || "None"}</dd>
                  </div>
                  <div>
                    <dt>Subagents</dt>
                    <dd>{product.profileDetail.subagents.join(", ") || "None"}</dd>
                  </div>
                </dl>
              ) : null}
            </div>
          ) : null}

          {section === "runtime" && draft !== undefined ? (
            <div className="settings-section">
              <div className="settings-section-heading">
                <div>
                  <h3>Profile details</h3>
                  <p>
                    These closed fields affect future runs; active runs keep their pinned snapshot.
                  </p>
                </div>
                <select
                  aria-label="Profile to edit"
                  value={selectedProfile?.name ?? ""}
                  onChange={(event) => setEditingProfileName(event.currentTarget.value)}
                >
                  {draft.profiles.map((profile) => (
                    <option key={profile.name} value={profile.name}>
                      {profile.name}
                    </option>
                  ))}
                </select>
              </div>
              {selectedProfile ? (
                <>
                  <label className="settings-field">
                    <span>Profile identity</span>
                    <input value={selectedProfile.name} readOnly />
                    <small>
                      Names stay stable so host-owned MCP and subagent bindings are retained.
                    </small>
                  </label>
                  <label className="settings-field">
                    <span>Model ID</span>
                    <input
                      value={selectedProfile.modelId}
                      maxLength={256}
                      onChange={(event) => {
                        const value = event.currentTarget.value;
                        updateProfile(selectedProfile.name, (profile) => ({
                          ...profile,
                          modelId: value,
                        }));
                      }}
                    />
                  </label>
                  <div className="settings-field-row">
                    <label className="settings-field">
                      <span>Model settings preset</span>
                      <input
                        value={selectedProfile.modelSettings ?? ""}
                        maxLength={128}
                        onChange={(event) => {
                          const value = event.currentTarget.value;
                          updateProfile(selectedProfile.name, (profile) => ({
                            ...profile,
                            ...(value ? { modelSettings: value } : { modelSettings: undefined }),
                          }));
                        }}
                      />
                    </label>
                    <label className="settings-field">
                      <span>Model config preset</span>
                      <input
                        value={selectedProfile.modelConfig ?? ""}
                        maxLength={128}
                        onChange={(event) => {
                          const value = event.currentTarget.value;
                          updateProfile(selectedProfile.name, (profile) => ({
                            ...profile,
                            ...(value ? { modelConfig: value } : { modelConfig: undefined }),
                          }));
                        }}
                      />
                    </label>
                  </div>
                  <label className="settings-field">
                    <span>Instructions, one per line</span>
                    <textarea
                      rows={5}
                      value={selectedProfile.instructions.join("\n")}
                      onChange={(event) => {
                        const value = event.currentTarget.value;
                        updateProfile(selectedProfile.name, (profile) => ({
                          ...profile,
                          instructions: lines(value),
                        }));
                      }}
                    />
                  </label>
                  <label className="settings-field">
                    <span>Toolsets, comma separated</span>
                    <input
                      value={selectedProfile.toolsets.join(", ")}
                      onChange={(event) => {
                        const value = event.currentTarget.value;
                        updateProfile(selectedProfile.name, (profile) => ({
                          ...profile,
                          toolsets: commaList(value),
                        }));
                      }}
                    />
                  </label>
                </>
              ) : null}
            </div>
          ) : null}

          {section === "providers" && draft !== undefined ? (
            <div className="settings-section">
              <div className="settings-section-heading">
                <div>
                  <h3>Provider routes</h3>
                  <p>Desktop edits safe routing metadata only. Credentials never enter this UI.</p>
                </div>
              </div>
              {draft.providers.length === 0 ? (
                <p className="settings-empty">No public provider routes are configured.</p>
              ) : (
                draft.providers.map((provider) => (
                  <fieldset className="provider-card" key={provider.name}>
                    <legend>{provider.name}</legend>
                    <label className="provider-toggle">
                      <input
                        type="checkbox"
                        checked={provider.enabled}
                        onChange={(event) => {
                          const checked = event.currentTarget.checked;
                          updateProvider(provider.name, (entry) => ({
                            ...entry,
                            enabled: checked,
                          }));
                        }}
                      />
                      Enabled for materialization
                    </label>
                    <label className="settings-field">
                      <span>Base URL</span>
                      <input
                        value={provider.baseUrl ?? ""}
                        maxLength={2048}
                        placeholder="Host default"
                        onChange={(event) => {
                          const value = event.currentTarget.value;
                          updateProvider(provider.name, (entry) => ({
                            ...entry,
                            ...(value ? { baseUrl: value } : { baseUrl: undefined }),
                          }));
                        }}
                      />
                    </label>
                    <label className="settings-field">
                      <span>Endpoint path</span>
                      <input
                        value={provider.endpointPath ?? ""}
                        maxLength={1024}
                        placeholder="Host default"
                        onChange={(event) => {
                          const value = event.currentTarget.value;
                          updateProvider(provider.name, (entry) => ({
                            ...entry,
                            ...(value ? { endpointPath: value } : { endpointPath: undefined }),
                          }));
                        }}
                      />
                    </label>
                  </fieldset>
                ))
              )}
            </div>
          ) : null}

          {section === "source" ? (
            <div className="settings-section">
              <h3>Runtime source</h3>
              <p>
                Preview the authoritative host source before reloading it. The preview is bound to
                the exact candidate and must still match when committed.
              </p>
              <dl className="profile-facts">
                <div>
                  <dt>Active generation</dt>
                  <dd>{product.runtimeConfigStatus?.active.generation ?? "Unavailable"}</dd>
                </div>
                <div>
                  <dt>Restart required</dt>
                  <dd>{product.runtimeConfigStatus?.restartRequired ? "Yes" : "No"}</dd>
                </div>
              </dl>
              <div className="settings-inline-actions">
                <button
                  type="button"
                  disabled={product.settingsLoading || product.busy}
                  onClick={() => void product.previewRuntimeReload()}
                >
                  Preview source
                </button>
                {product.reloadCandidateEtag !== undefined ? (
                  <button
                    type="button"
                    className="settings-primary"
                    disabled={product.busy || product.runtimeConfigValidation?.valid !== true}
                    onClick={() => void product.commitRuntimeReload()}
                  >
                    Reload reviewed source
                  </button>
                ) : null}
              </div>
              {product.runtimeConfigStatus?.restartRequired ? (
                <div className="settings-staged warning">
                  <strong>Restart-required settings are staged</strong>
                  <p>
                    Activation is owned by the managed runtime update milestone. You can safely
                    discard this candidate now.
                  </p>
                  <button
                    type="button"
                    disabled={product.busy}
                    onClick={() => void product.discardStagedRuntimeConfig()}
                  >
                    Discard staged settings
                  </button>
                </div>
              ) : null}
            </div>
          ) : null}

          {section !== "appearance" && product.runtimeConfigValidation !== undefined ? (
            <div
              className={
                product.runtimeConfigValidation.valid
                  ? "settings-validation"
                  : "settings-validation warning"
              }
              role="status"
            >
              <strong>
                {product.runtimeConfigValidation.valid
                  ? "Configuration is valid"
                  : "Changes need attention"}
              </strong>
              {product.runtimeConfigValidation.changedCategories.length > 0 ? (
                <p>Changes: {product.runtimeConfigValidation.changedCategories.join(", ")}</p>
              ) : null}
              {product.runtimeConfigValidation.issues.map((issue) => (
                <p key={`${issue.category}:${issue.code}`}>
                  {issue.severity}: {issue.message}
                </p>
              ))}
            </div>
          ) : null}
        </section>
      </div>

      <footer className="settings-footer">
        {section === "appearance" ? (
          <span>
            {desktopPreferences?.saving
              ? "Saving private Desktop preferences…"
              : desktopPreferences?.snapshot
                ? `Desktop preferences revision ${desktopPreferences.snapshot.revision}`
                : "Desktop preferences are not loaded"}
          </span>
        ) : (
          <>
            <span>
              {dirty ? "Unsaved runtime changes" : "Settings match the active safe projection"}
            </span>
            <div>
              <button
                type="button"
                disabled={draft === undefined || product.settingsLoading}
                onClick={() => draft && void product.validateRuntimeConfig(draft)}
              >
                Validate
              </button>
              <button
                type="button"
                className="settings-primary"
                disabled={draft === undefined || !dirty || product.busy || product.settingsLoading}
                onClick={() => draft && void product.saveRuntimeConfig(draft)}
              >
                Save changes
              </button>
            </div>
          </>
        )}
      </footer>
    </aside>
  );
}
