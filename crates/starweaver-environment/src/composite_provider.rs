//! Composite environment provider for SDK-level multi-environment routing.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write as _,
    sync::{Arc, Mutex},
};

use crate::{
    is_provider_visible_absolute_path, path_match_candidates as default_path_match_candidates,
    provider_visible_path_allowed_by_context, push_unique_candidate, DynEnvironmentProvider,
    DynProcessShellProvider, EnvironmentError, EnvironmentProvider, EnvironmentResult,
    EnvironmentState, FileGlobMatch, FileGlobOptions, FileGrepMatch, FileGrepOptions,
    FileListOptions, FileListResult, FileStat, ProcessShellProvider, ShellCommand, ShellOutput,
    ShellProcessSnapshot, ShellReviewEnvironmentContext,
};
use async_trait::async_trait;

const RESERVED_ROOT: &str = "environment";
const DEFAULT_COMPOSITE_ID: &str = "composite";

/// Access mode for a mounted environment.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum EnvironmentMountMode {
    /// File reads are allowed; mutation and shell execution are denied.
    ReadOnly,
    /// File reads, file writes, and shell execution are allowed.
    #[default]
    ReadWrite,
}

impl EnvironmentMountMode {
    const fn as_str(self) -> &'static str {
        match self {
            Self::ReadOnly => "read_only",
            Self::ReadWrite => "read_write",
        }
    }

    const fn permits_write(self) -> bool {
        matches!(self, Self::ReadWrite)
    }
}

/// One mounted environment behind a [`CompositeEnvironmentProvider`].
#[derive(Clone)]
pub struct EnvironmentMount {
    /// Stable agent-facing mount identity.
    pub id: String,
    /// Agent-facing virtual root, normally `/environment/{id}`.
    pub agent_root: String,
    /// Requested access mode.
    pub mode: EnvironmentMountMode,
    /// Mounted provider.
    pub provider: DynEnvironmentProvider,
    /// Whether this mount handles unqualified paths and `/`.
    pub is_default: bool,
    /// Whether this mount handles shell calls without an explicit routed `cwd`.
    pub default_for_shell: bool,
}

impl EnvironmentMount {
    /// Create a mounted environment.
    ///
    /// # Errors
    ///
    /// Returns an error when `id` is not a valid agent-facing mount identity.
    pub fn new(id: impl Into<String>, provider: DynEnvironmentProvider) -> EnvironmentResult<Self> {
        let id = id.into();
        validate_mount_id(&id)?;
        Ok(Self {
            agent_root: environment_root(&id),
            id,
            mode: EnvironmentMountMode::ReadWrite,
            provider,
            is_default: false,
            default_for_shell: false,
        })
    }

    /// Set access mode.
    #[must_use]
    pub const fn with_mode(mut self, mode: EnvironmentMountMode) -> Self {
        self.mode = mode;
        self
    }

    /// Mark this mount as the default file mount.
    #[must_use]
    pub const fn with_default(mut self, is_default: bool) -> Self {
        self.is_default = is_default;
        self
    }

    /// Mark this mount as the default shell mount.
    #[must_use]
    pub const fn with_default_for_shell(mut self, default_for_shell: bool) -> Self {
        self.default_for_shell = default_for_shell;
        self
    }
}

/// SDK-facing provider that routes one logical namespace to multiple providers.
pub struct CompositeEnvironmentProvider {
    id: String,
    mounts: Vec<EnvironmentMount>,
    default_index: usize,
    default_shell_index: Option<usize>,
    process_routes: Mutex<BTreeMap<String, ProcessRoute>>,
}

#[derive(Clone)]
struct RoutedProvider {
    id: String,
    agent_root: String,
    mode: EnvironmentMountMode,
    provider: DynEnvironmentProvider,
    child_path: String,
    explicit_root: bool,
}

#[derive(Clone, Debug)]
struct ProcessRoute {
    mount_id: String,
    child_process_id: String,
}

enum PathRoute {
    ReservedRoot,
    Provider(RoutedProvider),
}

impl CompositeEnvironmentProvider {
    /// Create a composite provider from mounted environments.
    ///
    /// # Errors
    ///
    /// Returns an error when mount identities are invalid or the default rules
    /// are ambiguous.
    pub fn new(mounts: Vec<EnvironmentMount>) -> EnvironmentResult<Self> {
        Self::with_id(DEFAULT_COMPOSITE_ID, mounts)
    }

    /// Create a composite provider with a custom provider id.
    ///
    /// # Errors
    ///
    /// Returns an error when mount identities are invalid or the default rules
    /// are ambiguous.
    pub fn with_id(
        id: impl Into<String>,
        mut mounts: Vec<EnvironmentMount>,
    ) -> EnvironmentResult<Self> {
        if mounts.is_empty() {
            return Err(EnvironmentError::InvalidRequest(
                "composite environment requires at least one mount".to_string(),
            ));
        }
        let mut seen = BTreeSet::<String>::new();
        for mount in &mut mounts {
            validate_mount_id(&mount.id)?;
            mount.agent_root = environment_root(&mount.id);
            if !seen.insert(mount.id.clone()) {
                return Err(EnvironmentError::InvalidRequest(format!(
                    "duplicate environment mount id: {}",
                    mount.id
                )));
            }
        }

        let default_indices = mounts
            .iter()
            .enumerate()
            .filter_map(|(index, mount)| mount.is_default.then_some(index))
            .collect::<Vec<_>>();
        let default_index = match default_indices.as_slice() {
            [] if mounts.len() == 1 => {
                mounts[0].is_default = true;
                0
            }
            [index] => *index,
            [] => {
                return Err(EnvironmentError::InvalidRequest(
                    "multiple environment mounts require exactly one default mount".to_string(),
                ));
            }
            _ => {
                return Err(EnvironmentError::InvalidRequest(
                    "only one environment mount can be default".to_string(),
                ));
            }
        };

        let default_shell_indices = mounts
            .iter()
            .enumerate()
            .filter_map(|(index, mount)| mount.default_for_shell.then_some(index))
            .collect::<Vec<_>>();
        let default_shell_index = match default_shell_indices.as_slice() {
            [] => {
                if mount_supports_shell_default(&mounts[default_index]) {
                    mounts[default_index].default_for_shell = true;
                    Some(default_index)
                } else {
                    None
                }
            }
            [index] => {
                if !mount_supports_shell_default(&mounts[*index]) {
                    return Err(EnvironmentError::InvalidRequest(format!(
                        "environment mount cannot be default_for_shell: {}",
                        mounts[*index].id
                    )));
                }
                Some(*index)
            }
            _ => {
                return Err(EnvironmentError::InvalidRequest(
                    "only one environment mount can be default_for_shell".to_string(),
                ));
            }
        };

        Ok(Self {
            id: id.into(),
            mounts,
            default_index,
            default_shell_index,
            process_routes: Mutex::new(BTreeMap::new()),
        })
    }

    fn default_mount(&self) -> &EnvironmentMount {
        &self.mounts[self.default_index]
    }

    fn default_shell_mount(&self) -> EnvironmentResult<&EnvironmentMount> {
        self.default_shell_index
            .map(|index| &self.mounts[index])
            .ok_or_else(|| {
                EnvironmentError::InvalidRequest(
                    "no default shell-capable environment mount is available".to_string(),
                )
            })
    }

    fn mount_by_id(&self, id: &str) -> EnvironmentResult<&EnvironmentMount> {
        self.mounts
            .iter()
            .find(|mount| mount.id == id)
            .ok_or_else(|| EnvironmentError::NotFound(format!("environment mount: {id}")))
    }

    fn route_path(&self, path: &str) -> EnvironmentResult<PathRoute> {
        let normalized = normalize_agent_path(path)?;
        let mut parts = normalized.split('/').filter(|part| !part.is_empty());
        if is_absolute_environment_namespace_path(path) && parts.next() == Some(RESERVED_ROOT) {
            let Some(id) = parts.next() else {
                return Ok(PathRoute::ReservedRoot);
            };
            let mount = self.mount_by_id(id)?;
            let child_path = parts.collect::<Vec<_>>().join("/");
            return Ok(PathRoute::Provider(RoutedProvider {
                id: mount.id.clone(),
                agent_root: mount.agent_root.clone(),
                mode: mount.mode,
                provider: mount.provider.clone(),
                child_path: provider_root_or_path(&child_path),
                explicit_root: true,
            }));
        }
        let mount = self.default_mount();
        let child_path = if default_mount_allows_provider_visible_path(mount, path, &normalized) {
            path.replace('\\', "/")
        } else {
            provider_root_or_path(&normalized)
        };
        Ok(PathRoute::Provider(RoutedProvider {
            id: mount.id.clone(),
            agent_root: mount.agent_root.clone(),
            mode: mount.mode,
            provider: mount.provider.clone(),
            child_path,
            explicit_root: false,
        }))
    }

    fn route_shell(&self, cwd: Option<&str>) -> EnvironmentResult<RoutedProvider> {
        if let Some(cwd) = cwd {
            if let Some(route) = self.route_provider_visible_shell_cwd(cwd)? {
                return Ok(route);
            }
            if let PathRoute::Provider(route) = self.route_path(cwd)? {
                return Ok(route);
            }
            return Err(EnvironmentError::InvalidRequest(
                "shell cwd cannot be the environment mount root".to_string(),
            ));
        }
        let mount = self.default_shell_mount()?;
        Ok(RoutedProvider {
            id: mount.id.clone(),
            agent_root: mount.agent_root.clone(),
            mode: mount.mode,
            provider: mount.provider.clone(),
            child_path: ".".to_string(),
            explicit_root: false,
        })
    }

    fn route_provider_visible_shell_cwd(
        &self,
        cwd: &str,
    ) -> EnvironmentResult<Option<RoutedProvider>> {
        if !is_provider_visible_absolute_path(cwd) || is_absolute_environment_namespace_path(cwd) {
            return Ok(None);
        }
        let mount = self.default_shell_mount()?;
        let context = mount.provider.shell_review_context();
        if !provider_visible_path_allowed_by_context(&context, cwd) {
            return Ok(None);
        }
        Ok(Some(RoutedProvider {
            id: mount.id.clone(),
            agent_root: mount.agent_root.clone(),
            mode: mount.mode,
            provider: mount.provider.clone(),
            child_path: cwd.to_string(),
            explicit_root: false,
        }))
    }

    fn route_for_process(&self, process_id: &str) -> EnvironmentResult<(EnvironmentMount, String)> {
        let stored_route = self
            .process_routes
            .lock()
            .map_err(|error| EnvironmentError::Provider(error.to_string()))?
            .get(process_id)
            .cloned();
        if let Some(route) = stored_route {
            return Ok((
                self.mount_by_id(&route.mount_id)?.clone(),
                route.child_process_id,
            ));
        }
        if let Some((mount_id, child_process_id)) = process_id.split_once(':') {
            return Ok((
                self.mount_by_id(mount_id)?.clone(),
                child_process_id.to_string(),
            ));
        }
        Ok((self.default_shell_mount()?.clone(), process_id.to_string()))
    }

    fn reserved_root_entries(&self) -> Vec<String> {
        self.mounts
            .iter()
            .map(|mount| mount.agent_root.clone())
            .collect()
    }
}

#[async_trait]
impl EnvironmentProvider for CompositeEnvironmentProvider {
    fn id(&self) -> &str {
        &self.id
    }

    fn process_shell_provider(self: Arc<Self>) -> Option<DynProcessShellProvider> {
        if self
            .mounts
            .iter()
            .any(|mount| mount.provider.clone().process_shell_provider().is_some())
        {
            Some(self)
        } else {
            None
        }
    }

    fn shell_review_context(&self) -> ShellReviewEnvironmentContext {
        let mut context = self.default_shell_mount().map_or_else(
            |_| ShellReviewEnvironmentContext::default(),
            |mount| mount.provider.shell_review_context(),
        );
        for mount in &self.mounts {
            if !context
                .allowed_paths
                .iter()
                .any(|existing| existing == &mount.agent_root)
            {
                context.allowed_paths.push(mount.agent_root.clone());
            }
        }
        context
    }

    async fn read_text(&self, path: &str) -> EnvironmentResult<String> {
        let route = expect_provider_route(self.route_path(path)?)?;
        route.provider.read_text(&route.child_path).await
    }

    async fn read_bytes(
        &self,
        path: &str,
        offset: usize,
        length: Option<usize>,
    ) -> EnvironmentResult<Vec<u8>> {
        let route = expect_provider_route(self.route_path(path)?)?;
        route
            .provider
            .read_bytes(&route.child_path, offset, length)
            .await
    }

    async fn write_text(&self, path: &str, content: &str) -> EnvironmentResult<()> {
        let route = expect_provider_route(self.route_path(path)?)?;
        ensure_write(&route, "write")?;
        route.provider.write_text(&route.child_path, content).await
    }

    async fn create_dir(&self, path: &str, parents: bool) -> EnvironmentResult<()> {
        let route = expect_provider_route(self.route_path(path)?)?;
        ensure_write(&route, "create directory")?;
        route.provider.create_dir(&route.child_path, parents).await
    }

    async fn delete_path(&self, path: &str, recursive: bool) -> EnvironmentResult<()> {
        let route = expect_provider_route(self.route_path(path)?)?;
        ensure_write(&route, "delete")?;
        route
            .provider
            .delete_path(&route.child_path, recursive)
            .await
    }

    async fn move_path(&self, src: &str, dst: &str, overwrite: bool) -> EnvironmentResult<()> {
        let src_route = expect_provider_route(self.route_path(src)?)?;
        let dst_route = expect_provider_route(self.route_path(dst)?)?;
        ensure_write(&src_route, "move")?;
        ensure_write(&dst_route, "move")?;
        if src_route.id == dst_route.id {
            return src_route
                .provider
                .move_path(&src_route.child_path, &dst_route.child_path, overwrite)
                .await;
        }
        cross_mount_copy(&src_route, &dst_route, overwrite).await?;
        src_route
            .provider
            .delete_path(&src_route.child_path, false)
            .await
    }

    async fn copy_path(&self, src: &str, dst: &str, overwrite: bool) -> EnvironmentResult<()> {
        let src_route = expect_provider_route(self.route_path(src)?)?;
        let dst_route = expect_provider_route(self.route_path(dst)?)?;
        ensure_write(&dst_route, "copy")?;
        if src_route.id == dst_route.id {
            return src_route
                .provider
                .copy_path(&src_route.child_path, &dst_route.child_path, overwrite)
                .await;
        }
        cross_mount_copy(&src_route, &dst_route, overwrite).await
    }

    async fn write_tmp_file(&self, filename: &str, content: &[u8]) -> EnvironmentResult<String> {
        let mount = self.default_mount();
        let route = RoutedProvider {
            id: mount.id.clone(),
            agent_root: mount.agent_root.clone(),
            mode: mount.mode,
            provider: mount.provider.clone(),
            child_path: ".".to_string(),
            explicit_root: false,
        };
        ensure_write(&route, "write temporary file")?;
        mount.provider.write_tmp_file(filename, content).await
    }

    async fn stat(&self, path: &str) -> EnvironmentResult<FileStat> {
        match self.route_path(path)? {
            PathRoute::ReservedRoot => Ok(FileStat {
                size: 0,
                is_file: false,
                is_dir: true,
                modified_unix_seconds: None,
            }),
            PathRoute::Provider(route) => {
                if route.explicit_root && route.child_path == "." {
                    return Ok(FileStat {
                        size: 0,
                        is_file: false,
                        is_dir: true,
                        modified_unix_seconds: None,
                    });
                }
                route.provider.stat(&route.child_path).await
            }
        }
    }

    async fn list(&self, path: &str) -> EnvironmentResult<Vec<String>> {
        match self.route_path(path)? {
            PathRoute::ReservedRoot => Ok(self.reserved_root_entries()),
            PathRoute::Provider(route) => {
                let entries = route.provider.list(&route.child_path).await?;
                Ok(entries
                    .into_iter()
                    .map(|entry| rebase_list_entry(&route, &entry))
                    .collect())
            }
        }
    }

    async fn list_with_options(
        &self,
        path: &str,
        options: FileListOptions,
    ) -> EnvironmentResult<FileListResult> {
        match self.route_path(path)? {
            PathRoute::ReservedRoot => {
                let entries = self.reserved_root_entries();
                let total_entries = entries.len();
                let truncated = options.max_entries > 0 && total_entries > options.max_entries;
                let entries = if truncated {
                    entries.into_iter().take(options.max_entries).collect()
                } else {
                    entries
                };
                Ok(FileListResult {
                    entries,
                    truncated,
                    total_entries,
                })
            }
            PathRoute::Provider(route) => {
                let mut result = route
                    .provider
                    .list_with_options(&route.child_path, options)
                    .await?;
                result.entries = result
                    .entries
                    .into_iter()
                    .map(|entry| rebase_list_entry(&route, &entry))
                    .collect();
                Ok(result)
            }
        }
    }

    fn path_match_candidates(&self, path: &str) -> Vec<String> {
        let mut candidates = default_path_match_candidates(path);
        if let Ok(PathRoute::Provider(route)) = self.route_path(path) {
            for candidate in route.provider.path_match_candidates(&route.child_path) {
                push_unique_candidate(&mut candidates, rebase_match_candidate(&route, &candidate));
                push_unique_candidate(&mut candidates, candidate);
            }
        }
        candidates
    }

    async fn glob(
        &self,
        path: &str,
        pattern: &str,
        options: FileGlobOptions,
    ) -> EnvironmentResult<Vec<FileGlobMatch>> {
        let route = expect_provider_route(self.route_path(path)?)?;
        let matches = route
            .provider
            .glob(&route.child_path, pattern, options)
            .await?;
        Ok(matches
            .into_iter()
            .map(|entry| FileGlobMatch {
                path: rebase_child_path(&route, &entry.path),
            })
            .collect())
    }

    async fn grep(
        &self,
        path: &str,
        pattern: &str,
        options: FileGrepOptions,
    ) -> EnvironmentResult<Vec<FileGrepMatch>> {
        let route = expect_provider_route(self.route_path(path)?)?;
        let matches = route
            .provider
            .grep(&route.child_path, pattern, options)
            .await?;
        Ok(matches
            .into_iter()
            .map(|mut entry| {
                entry.path = rebase_child_path(&route, &entry.path);
                entry
            })
            .collect())
    }

    async fn run_shell(&self, command: ShellCommand) -> EnvironmentResult<ShellOutput> {
        let route = self.route_shell(command.cwd.as_deref())?;
        ensure_write(&route, "run shell")?;
        let child_command = child_shell_command(command, &route);
        route.provider.run_shell(child_command).await
    }

    async fn render_environment_context(&self) -> EnvironmentResult<Option<String>> {
        let mount_summary = render_mount_summary(&self.mounts);
        let default_context = self
            .default_mount()
            .provider
            .render_environment_context()
            .await?
            .unwrap_or_default();
        if default_context.trim().is_empty() {
            return Ok(Some(mount_summary));
        }
        Ok(Some(format!(
            "{mount_summary}\n<default-environment-context>\n{default_context}\n</default-environment-context>"
        )))
    }

    async fn export_state(&self) -> EnvironmentResult<EnvironmentState> {
        let mut state = self.default_mount().provider.export_state().await?;
        state.provider_id.clone_from(&self.id);
        state.metadata.insert(
            crate::ENVIRONMENT_PROVIDER_KIND_KEY.to_string(),
            serde_json::json!("composite"),
        );
        state.metadata.insert(
            "mounts".to_string(),
            serde_json::json!(self
                .mounts
                .iter()
                .map(|mount| {
                    serde_json::json!({
                        "id": mount.id,
                        "root": mount.agent_root,
                        "mode": mount.mode.as_str(),
                        "default": mount.is_default,
                        "default_for_shell": mount.default_for_shell,
                    })
                })
                .collect::<Vec<_>>()),
        );
        state.processes = self.list_processes().await.unwrap_or_default();
        Ok(state)
    }
}

#[async_trait]
impl ProcessShellProvider for CompositeEnvironmentProvider {
    async fn start_process(
        &self,
        command: ShellCommand,
    ) -> EnvironmentResult<ShellProcessSnapshot> {
        let route = self.route_shell(command.cwd.as_deref())?;
        ensure_write(&route, "start process")?;
        let process_provider =
            route
                .provider
                .clone()
                .process_shell_provider()
                .ok_or_else(|| {
                    EnvironmentError::InvalidRequest(format!(
                        "environment mount does not support background processes: {}",
                        route.id
                    ))
                })?;
        let child_command = child_shell_command(command, &route);
        let snapshot = process_provider.start_process(child_command).await?;
        let child_process_id = snapshot.process_id.clone();
        let snapshot = rebase_process_snapshot(&route, snapshot);
        self.process_routes
            .lock()
            .map_err(|error| EnvironmentError::Provider(error.to_string()))?
            .insert(
                snapshot.process_id.clone(),
                ProcessRoute {
                    mount_id: route.id,
                    child_process_id,
                },
            );
        Ok(snapshot)
    }

    async fn wait_process(
        &self,
        process_id: &str,
        timeout_seconds: u64,
    ) -> EnvironmentResult<ShellProcessSnapshot> {
        let (mount, child_process_id) = self.route_for_process(process_id)?;
        let process_provider = process_provider_for_mount(&mount)?;
        let snapshot = process_provider
            .wait_process(&child_process_id, timeout_seconds)
            .await?;
        Ok(rebase_process_snapshot(&route_from_mount(&mount), snapshot))
    }

    async fn list_processes(&self) -> EnvironmentResult<Vec<ShellProcessSnapshot>> {
        let mut processes = Vec::new();
        for mount in &self.mounts {
            let Some(provider) = mount.provider.clone().process_shell_provider() else {
                continue;
            };
            let route = route_from_mount(mount);
            processes.extend(
                provider
                    .list_processes()
                    .await?
                    .into_iter()
                    .map(|snapshot| rebase_process_snapshot(&route, snapshot)),
            );
        }
        Ok(processes)
    }

    async fn input_process(
        &self,
        process_id: &str,
        text: &str,
        close_stdin: bool,
    ) -> EnvironmentResult<ShellProcessSnapshot> {
        let (mount, child_process_id) = self.route_for_process(process_id)?;
        ensure_mount_write(&mount, "input process")?;
        let process_provider = process_provider_for_mount(&mount)?;
        let snapshot = process_provider
            .input_process(&child_process_id, text, close_stdin)
            .await?;
        Ok(rebase_process_snapshot(&route_from_mount(&mount), snapshot))
    }

    async fn signal_process(
        &self,
        process_id: &str,
        signal: i32,
    ) -> EnvironmentResult<ShellProcessSnapshot> {
        let (mount, child_process_id) = self.route_for_process(process_id)?;
        ensure_mount_write(&mount, "signal process")?;
        let process_provider = process_provider_for_mount(&mount)?;
        let snapshot = process_provider
            .signal_process(&child_process_id, signal)
            .await?;
        Ok(rebase_process_snapshot(&route_from_mount(&mount), snapshot))
    }

    async fn kill_process(&self, process_id: &str) -> EnvironmentResult<ShellProcessSnapshot> {
        let (mount, child_process_id) = self.route_for_process(process_id)?;
        ensure_mount_write(&mount, "kill process")?;
        let process_provider = process_provider_for_mount(&mount)?;
        let snapshot = process_provider.kill_process(&child_process_id).await?;
        Ok(rebase_process_snapshot(&route_from_mount(&mount), snapshot))
    }
}

async fn cross_mount_copy(
    src_route: &RoutedProvider,
    dst_route: &RoutedProvider,
    overwrite: bool,
) -> EnvironmentResult<()> {
    let src_stat = src_route.provider.stat(&src_route.child_path).await?;
    if src_stat.is_dir {
        return Err(EnvironmentError::InvalidRequest(
            "cross-mount directory copy is not supported".to_string(),
        ));
    }
    if !overwrite {
        match dst_route.provider.stat(&dst_route.child_path).await {
            Ok(_) => {
                return Err(EnvironmentError::InvalidRequest(format!(
                    "destination already exists: {}",
                    rebase_child_path(dst_route, &dst_route.child_path)
                )));
            }
            Err(EnvironmentError::NotFound(_)) => {}
            Err(error) => return Err(error),
        }
    }
    let content = src_route.provider.read_text(&src_route.child_path).await?;
    dst_route
        .provider
        .write_text(&dst_route.child_path, &content)
        .await
}

fn process_provider_for_mount(
    mount: &EnvironmentMount,
) -> EnvironmentResult<DynProcessShellProvider> {
    mount
        .provider
        .clone()
        .process_shell_provider()
        .ok_or_else(|| {
            EnvironmentError::InvalidRequest(format!(
                "environment mount does not support background processes: {}",
                mount.id
            ))
        })
}

fn mount_supports_shell_default(mount: &EnvironmentMount) -> bool {
    mount.mode.permits_write() && mount.provider.clone().process_shell_provider().is_some()
}

fn route_from_mount(mount: &EnvironmentMount) -> RoutedProvider {
    RoutedProvider {
        id: mount.id.clone(),
        agent_root: mount.agent_root.clone(),
        mode: mount.mode,
        provider: mount.provider.clone(),
        child_path: ".".to_string(),
        explicit_root: !mount.is_default,
    }
}

fn child_shell_command(mut command: ShellCommand, route: &RoutedProvider) -> ShellCommand {
    if command.cwd.is_some() {
        command.cwd = Some(route.child_path.clone());
    }
    command
}

fn expect_provider_route(route: PathRoute) -> EnvironmentResult<RoutedProvider> {
    match route {
        PathRoute::Provider(route) => Ok(route),
        PathRoute::ReservedRoot => Err(EnvironmentError::InvalidRequest(
            "operation requires a concrete environment mount".to_string(),
        )),
    }
}

fn ensure_write(route: &RoutedProvider, action: &str) -> EnvironmentResult<()> {
    if route.mode.permits_write() {
        Ok(())
    } else {
        Err(EnvironmentError::AccessDenied(format!(
            "{action} denied for read-only environment mount: {}",
            route.id
        )))
    }
}

fn ensure_mount_write(mount: &EnvironmentMount, action: &str) -> EnvironmentResult<()> {
    if mount.mode.permits_write() {
        Ok(())
    } else {
        Err(EnvironmentError::AccessDenied(format!(
            "{action} denied for read-only environment mount: {}",
            mount.id
        )))
    }
}

fn rebase_child_path(route: &RoutedProvider, child_path: &str) -> String {
    if !route.explicit_root {
        return child_path.to_string();
    }
    let normalized = normalize_agent_path(child_path).unwrap_or_else(|_| child_path.to_string());
    if normalized.is_empty() || normalized == "." {
        route.agent_root.clone()
    } else {
        format!("{}/{}", route.agent_root, normalized)
    }
}

fn rebase_list_entry(route: &RoutedProvider, entry: &str) -> String {
    if !route.explicit_root {
        return entry.to_string();
    }
    let child_path =
        normalize_agent_path(&route.child_path).unwrap_or_else(|_| route.child_path.clone());
    let entry_path = normalize_agent_path(entry).unwrap_or_else(|_| entry.to_string());
    if child_path.is_empty() || child_path == "." {
        return rebase_child_path(route, &entry_path);
    }
    if entry_path == child_path || entry_path.starts_with(&format!("{child_path}/")) {
        rebase_child_path(route, &entry_path)
    } else if entry_path.is_empty() || entry_path == "." {
        rebase_child_path(route, &child_path)
    } else {
        rebase_child_path(route, &format!("{child_path}/{entry_path}"))
    }
}

fn rebase_match_candidate(route: &RoutedProvider, candidate: &str) -> String {
    if !route.explicit_root || is_provider_visible_absolute_path(candidate) {
        return candidate.replace('\\', "/");
    }
    rebase_child_path(route, candidate)
}

fn rebase_process_snapshot(
    route: &RoutedProvider,
    mut snapshot: ShellProcessSnapshot,
) -> ShellProcessSnapshot {
    snapshot.process_id = composite_process_id(&route.id, &snapshot.process_id);
    if let Some(cwd) = snapshot
        .metadata
        .get("cwd")
        .and_then(serde_json::Value::as_str)
    {
        if route.explicit_root {
            snapshot.metadata.insert(
                "cwd".to_string(),
                serde_json::json!(rebase_child_path(route, cwd)),
            );
        }
    }
    snapshot
}

fn composite_process_id(mount_id: &str, child_process_id: &str) -> String {
    format!("{mount_id}:{child_process_id}")
}

fn provider_root_or_path(path: &str) -> String {
    if path.is_empty() {
        ".".to_string()
    } else {
        path.to_string()
    }
}

fn environment_root(id: &str) -> String {
    format!("/{RESERVED_ROOT}/{id}")
}

fn is_absolute_environment_namespace_path(path: &str) -> bool {
    let path = path.replace('\\', "/");
    if !path.starts_with('/') {
        return false;
    }
    let trimmed = path.trim_start_matches('/');
    trimmed == RESERVED_ROOT
        || trimmed
            .strip_prefix(RESERVED_ROOT)
            .is_some_and(|rest| rest.starts_with('/'))
}

fn default_mount_allows_provider_visible_path(
    mount: &EnvironmentMount,
    path: &str,
    normalized: &str,
) -> bool {
    is_provider_visible_absolute_path(path)
        && !normalized.is_empty()
        && provider_visible_path_allowed_by_context(&mount.provider.shell_review_context(), path)
}

fn normalize_agent_path(path: &str) -> EnvironmentResult<String> {
    let path = path.replace('\\', "/");
    let trimmed = path.trim_start_matches('/');
    let mut parts = Vec::new();
    for part in trimmed.split('/') {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." {
            return Err(EnvironmentError::InvalidRequest(path));
        }
        parts.push(part);
    }
    Ok(parts.join("/"))
}

fn validate_mount_id(id: &str) -> EnvironmentResult<()> {
    if id.is_empty()
        || matches!(id, "." | ".." | RESERVED_ROOT)
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(EnvironmentError::InvalidRequest(format!(
            "invalid environment mount id: {id}"
        )));
    }
    Ok(())
}

fn render_mount_summary(mounts: &[EnvironmentMount]) -> String {
    let mut xml = String::from("<environment-mounts>\n");
    if let Some(default) = mounts.iter().find(|mount| mount.is_default) {
        let _ = writeln!(xml, "  <default mount=\"{}\" root=\"/\" />", default.id);
    }
    for mount in mounts {
        let command = if mount.mode.permits_write() {
            "run"
        } else {
            "none"
        };
        let process = if mount.mode.permits_write()
            && mount.provider.clone().process_shell_provider().is_some()
        {
            "background"
        } else {
            "none"
        };
        let _ = writeln!(
            xml,
            "  <mount id=\"{}\" root=\"{}\" mode=\"{}\" files=\"{}\" command=\"{}\" process=\"{}\" readiness=\"ready\" />",
            mount.id,
            mount.agent_root,
            mount.mode.as_str(),
            mount.mode.as_str(),
            command,
            process
        );
    }
    xml.push_str("</environment-mounts>");
    xml
}
