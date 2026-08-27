//! Provider, process, tmux, and Terminal.app integrations for Managed Sessions.

use crate::adapters::config::{configured_path, Config};
use crate::adapters::persistence::managed_sessions::ManagedLaunchStore;
use crate::adapters::providers::{registry, state, ProviderExecutables};
use crate::application::managed_sessions::{
    run_attach_from_args_with_access, BindingReceipt, ManagedCurrentRunAccessService,
    ManagedCurrentRunEvidence, ManagedCurrentRunLifecycle, ManagedCurrentRunRecord,
    ManagedCurrentRunTerminalAccess, ManagedCurrentRunVerifier, ManagedLaunchBroker,
    ManagedLaunchCleanup, ManagedLaunchEvidence, ManagedLaunchRegistration, ManagedProviderRuntime,
    ManagedResumeEvidenceCandidate, ManagedResumeProviderEvidence, ManagedSessionRecord,
    ManagedTerminalAccess, PrivateTmuxEnvironment, ProcessEvidence, ProviderNewCommand,
    ProviderProcessObservation, VerifiedManagedCurrentRun,
};
use crate::domain::{Provider, SessionState};
use anyhow::{anyhow, Context, Result};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

static REGISTRATION_SEQUENCE: AtomicU64 = AtomicU64::new(0);

impl ManagedProviderRuntime for ProviderExecutables {
    fn validate_target(
        &self,
        target: &crate::application::managed_sessions::ManagedLaunchTarget,
    ) -> Result<()> {
        if !target.working_directory.is_absolute() || !target.working_directory.is_dir() {
            return Err(anyhow!("New target working directory is unavailable"));
        }
        std::fs::read_dir(&target.working_directory)
            .context("New target working directory is unreadable")?;
        Ok(())
    }

    fn new_registration_id(&self, _now_ms: i64) -> String {
        let sequence = REGISTRATION_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        format!("launch-{:x}-{:x}-{:x}", nonce, std::process::id(), sequence)
    }

    fn new_session_command(
        &self,
        provider: Provider,
        binding_initializer: bool,
    ) -> Result<ProviderNewCommand> {
        let executable = executable_for(self, provider)?;
        Ok(ProviderNewCommand {
            executable,
            arguments: registry()
                .runtime_control(provider)?
                .new_session_args(binding_initializer),
        })
    }

    fn resume_session_command(
        &self,
        provider: Provider,
        native_session_id: &str,
    ) -> Result<ProviderNewCommand> {
        let executable = executable_for(self, provider)?;
        Ok(ProviderNewCommand {
            executable,
            arguments: registry()
                .runtime_control(provider)?
                .resume_session_args(native_session_id),
        })
    }
}

fn executable_for(executables: &ProviderExecutables, provider: Provider) -> Result<PathBuf> {
    let executable = executables
        .executable_for(provider)
        .ok_or_else(|| anyhow!("{} executable is unavailable", provider.label()))?
        .to_path_buf();
    validate_executable(&executable, provider.label())?;
    Ok(executable)
}

pub struct SystemProcessEvidence;

impl SystemProcessEvidence {
    pub fn current_identity(&self, pid: u32) -> Option<crate::domain::ProcessIdentity> {
        match self.observe(pid) {
            ProviderProcessObservation::Live(identity) => Some(identity),
            ProviderProcessObservation::Absent | ProviderProcessObservation::Unknown => None,
        }
    }
}

impl ProcessEvidence for SystemProcessEvidence {
    fn observe(&self, pid: u32) -> ProviderProcessObservation {
        process_observation(state::inspect_process_identity(pid))
    }

    fn observe_many(&self, pids: &[u32]) -> Vec<ProviderProcessObservation> {
        state::inspect_process_identities(pids)
            .into_iter()
            .map(process_observation)
            .collect()
    }
}

fn process_observation(inspection: state::ProcessInspection) -> ProviderProcessObservation {
    match inspection {
        state::ProcessInspection::Present(identity, false) => {
            ProviderProcessObservation::Live(identity)
        }
        state::ProcessInspection::Present(_, true) | state::ProcessInspection::Absent => {
            ProviderProcessObservation::Absent
        }
        state::ProcessInspection::Unknown => ProviderProcessObservation::Unknown,
    }
}

pub struct SystemManagedResumeProviderEvidence {
    config: Config,
}

impl SystemManagedResumeProviderEvidence {
    pub fn new(config: Config) -> Self {
        Self { config }
    }

    fn observe_with_detector(
        detector: &state::StateDetector,
        run: &ManagedCurrentRunRecord,
        session: &ManagedSessionRecord,
        receipt: &BindingReceipt,
    ) -> Result<ManagedCurrentRunLifecycle> {
        let source = match receipt.source.path.canonicalize() {
            Ok(source) if source.is_file() => source,
            _ => return Ok(ManagedCurrentRunLifecycle::Unknown),
        };
        let parsed = match registry().observation(run.provider)?.parse_file(&source) {
            Some(parsed) => parsed,
            None => return Ok(ManagedCurrentRunLifecycle::Unknown),
        };
        let parsed_working_directory = parsed
            .cwd
            .as_deref()
            .and_then(|cwd| Path::new(cwd).canonicalize().ok());
        let target_working_directory = session.target.working_directory.canonicalize().ok();
        if parsed.provider != run.provider
            || parsed.id != run.native_session_id
            || parsed_working_directory.is_none()
            || parsed_working_directory != target_working_directory
            || !detector.scan_complete(run.provider)
        {
            return Ok(ManagedCurrentRunLifecycle::Unknown);
        }
        let live = registry()
            .runtime_control(run.provider)?
            .observe_current_run(detector, &run.native_session_id, &source);
        if live.state.is_open() {
            Ok(ManagedCurrentRunLifecycle::Open)
        } else if live.state == SessionState::Exited && live.process_identity.is_none() {
            Ok(ManagedCurrentRunLifecycle::Exited)
        } else {
            Ok(ManagedCurrentRunLifecycle::Unknown)
        }
    }
}

impl ManagedResumeProviderEvidence for SystemManagedResumeProviderEvidence {
    fn observe(
        &self,
        run: &ManagedCurrentRunRecord,
        session: &ManagedSessionRecord,
        receipt: &BindingReceipt,
    ) -> Result<ManagedCurrentRunLifecycle> {
        let detector = state::StateDetector::gather(&self.config.claude_sessions_dir());
        Self::observe_with_detector(&detector, run, session, receipt)
    }

    fn observe_many(
        &self,
        candidates: &[ManagedResumeEvidenceCandidate],
    ) -> Vec<Result<ManagedCurrentRunLifecycle>> {
        if candidates.is_empty() {
            return Vec::new();
        }
        let detector = state::StateDetector::gather(&self.config.claude_sessions_dir());
        candidates
            .iter()
            .map(|candidate| {
                Self::observe_with_detector(
                    &detector,
                    &candidate.run,
                    &candidate.session,
                    &candidate.receipt,
                )
            })
            .collect()
    }
}

pub struct SystemManagedCurrentRunEvidence {
    config: Config,
    processes: SystemProcessEvidence,
}

impl SystemManagedCurrentRunEvidence {
    pub fn new(config: Config) -> Self {
        Self {
            config,
            processes: SystemProcessEvidence,
        }
    }
}

impl ManagedCurrentRunEvidence for SystemManagedCurrentRunEvidence {
    fn revalidate(
        &self,
        run: &ManagedCurrentRunRecord,
        session: &ManagedSessionRecord,
        receipt: &BindingReceipt,
    ) -> Result<()> {
        match self.processes.observe(run.provider_process.pid) {
            ProviderProcessObservation::Live(identity) if identity == run.provider_process => {}
            ProviderProcessObservation::Live(_) | ProviderProcessObservation::Absent => {
                return Err(anyhow!("Managed Current Run process identity is stale"));
            }
            ProviderProcessObservation::Unknown => {
                return Err(anyhow!(
                    "Managed Current Run process identity could not be verified"
                ));
            }
        }
        let evidence = registry()
            .runtime_control(run.provider)?
            .binding_evidence(&self.config, run.provider_process)
            .ok_or_else(|| anyhow!("Managed Current Run is not verified Open"))?;
        if evidence.provider != run.provider
            || evidence.source_session_id != run.native_session_id
            || evidence.process_session_id != run.native_session_id
            || evidence.provider_process != run.provider_process
            || evidence.source != receipt.source
            || evidence.working_directory != session.target.working_directory
        {
            return Err(anyhow!(
                "Managed Current Run live evidence no longer matches its Binding Receipt"
            ));
        }
        Ok(())
    }
}

pub struct TmuxManagedLaunchCleanup {
    tmux_executable: PathBuf,
}

impl TmuxManagedLaunchCleanup {
    pub fn new(tmux_executable: PathBuf) -> Self {
        Self { tmux_executable }
    }
}

impl ManagedLaunchCleanup for TmuxManagedLaunchCleanup {
    fn clean_empty(&self, registration: &ManagedLaunchRegistration) -> Result<()> {
        clean_empty_private_tmux(&self.tmux_executable, &registration.tmux)
    }
}

pub struct TmuxLaunchBroker {
    tmux_executable: PathBuf,
}

impl TmuxLaunchBroker {
    pub fn new(tmux_executable: PathBuf) -> Self {
        Self { tmux_executable }
    }
}

impl ManagedLaunchBroker for TmuxLaunchBroker {
    fn launch(
        &mut self,
        registration: &ManagedLaunchRegistration,
    ) -> Result<ManagedLaunchEvidence> {
        validate_executable(&self.tmux_executable, "tmux")?;
        validate_executable(
            &registration.provider_command.executable,
            registration.provider.label(),
        )?;
        validate_tmux_name(&registration.tmux.socket_name)?;
        validate_tmux_name(&registration.tmux.session_name)?;
        let terminal_title = escape_tmux_format(&registration.session_alias);
        let window_title = tmux_window_title(
            &registration.session_alias,
            registration.description.as_deref(),
        );
        let output = Command::new(&self.tmux_executable)
            .env_remove("TMUX")
            .args([OsStr::new("-f"), OsStr::new("/dev/null"), OsStr::new("-L")])
            .arg(&registration.tmux.socket_name)
            .args([
                OsStr::new("new-session"),
                OsStr::new("-d"),
                OsStr::new("-P"),
                OsStr::new("-F"),
                OsStr::new("#{pane_id}\t#{pane_pid}"),
                OsStr::new("-s"),
            ])
            .arg(&registration.tmux.session_name)
            .arg("-n")
            .arg(window_title)
            .arg("-c")
            .arg(&registration.target.working_directory)
            .arg("--")
            .arg(&registration.provider_command.executable)
            .args(&registration.provider_command.arguments)
            .arg(";")
            .args(["set-option", "-w", "automatic-rename", "off"])
            .arg(";")
            .args(["set-option", "-g", "set-titles", "on"])
            .arg(";")
            .args(["set-option", "-g", "set-titles-string"])
            .arg(&terminal_title)
            .arg(";")
            .args(["set-option", "-g", "mouse", "on"])
            .arg(";")
            .args(["set-option", "-s", "copy-command", "/usr/bin/pbcopy"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
            .context("could not start private tmux")?;
        if !output.status.success() {
            return Err(anyhow!("private tmux launch exited with {}", output.status));
        }
        let output = String::from_utf8(output.stdout)
            .map_err(|_| anyhow!("private tmux launch evidence is not valid UTF-8"))?;
        let (tmux_pane_id, pane_pid) = output
            .trim()
            .split_once('\t')
            .ok_or_else(|| anyhow!("private tmux launch evidence is incomplete"))?;
        validate_tmux_pane_id(tmux_pane_id)?;
        let pane_pid = pane_pid
            .parse::<u32>()
            .map_err(|_| anyhow!("private tmux Provider PID is invalid"))?;
        let provider_process = state::process_identity_after_spawn(pane_pid)
            .ok_or_else(|| anyhow!("private tmux Provider process identity is unavailable"))?;
        Ok(ManagedLaunchEvidence {
            tmux_pane_id: tmux_pane_id.to_string(),
            provider_process,
        })
    }

    fn cancel(&mut self, registration: &ManagedLaunchRegistration) -> Result<()> {
        terminate_private_tmux(&self.tmux_executable, &registration.tmux)
    }
}

pub struct MacosManagedTerminalAccess {
    tmux_executable: PathBuf,
    osascript_executable: PathBuf,
}

impl MacosManagedTerminalAccess {
    pub fn new(tmux_executable: PathBuf) -> Self {
        Self::with_osascript(tmux_executable, PathBuf::from("/usr/bin/osascript"))
    }

    pub fn with_osascript(tmux_executable: PathBuf, osascript_executable: PathBuf) -> Self {
        Self {
            tmux_executable,
            osascript_executable,
        }
    }

    fn open_private_tmux(&self, tmux: &PrivateTmuxEnvironment) -> Result<()> {
        validate_executable(&self.tmux_executable, "tmux")?;
        validate_tmux_name(&tmux.socket_name)?;
        validate_tmux_name(&tmux.session_name)?;
        let command = [
            shell_quote_os(self.tmux_executable.as_os_str())?,
            "-L".to_string(),
            shell_quote(&tmux.socket_name),
            "attach-session".to_string(),
            "-t".to_string(),
            shell_quote(&tmux.session_name),
        ]
        .join(" ");
        let status = Command::new(&self.osascript_executable)
            .args([
                OsStr::new("-e"),
                OsStr::new("on run argv"),
                OsStr::new("-e"),
                OsStr::new("tell application \"Terminal\""),
                OsStr::new("-e"),
                OsStr::new("do script (item 1 of argv)"),
                OsStr::new("-e"),
                OsStr::new("activate"),
                OsStr::new("-e"),
                OsStr::new("end tell"),
                OsStr::new("-e"),
                OsStr::new("end run"),
                OsStr::new("--"),
                OsStr::new(&command),
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .context("could not open Terminal.app")?;
        if status.success() {
            Ok(())
        } else {
            Err(anyhow!("Terminal.app launch failed"))
        }
    }
}

impl ManagedTerminalAccess for MacosManagedTerminalAccess {
    fn open(&mut self, registration: &ManagedLaunchRegistration) -> Result<()> {
        self.open_private_tmux(&registration.tmux)
    }
}

impl ManagedCurrentRunTerminalAccess for MacosManagedTerminalAccess {
    fn open_terminal(&mut self, run: &VerifiedManagedCurrentRun) -> Result<()> {
        self.open_private_tmux(&run.tmux)
    }

    fn attach_foreground(&mut self, run: &VerifiedManagedCurrentRun) -> Result<()> {
        validate_executable(&self.tmux_executable, "tmux")?;
        validate_tmux_name(&run.tmux.socket_name)?;
        validate_tmux_name(&run.tmux.session_name)?;
        let status = Command::new(&self.tmux_executable)
            .env_remove("TMUX")
            .args([
                OsStr::new("-L"),
                OsStr::new(&run.tmux.socket_name),
                OsStr::new("attach-session"),
                OsStr::new("-t"),
                OsStr::new(&run.tmux.session_name),
            ])
            .status()
            .context("could not attach to the Managed Current Run")?;
        if status.success() {
            Ok(())
        } else {
            Err(anyhow!("Managed Current Run attach failed"))
        }
    }
}

pub fn run_attach_from_args<I, S>(args: I) -> Result<bool>
where
    I: IntoIterator<Item = S>,
    S: Into<OsString>,
{
    let args = args.into_iter().map(Into::into).collect::<Vec<_>>();
    if args.get(1).map(OsString::as_os_str) != Some(OsStr::new("attach")) {
        return Ok(false);
    }
    let config = Config::load();
    let tmux_executable = discover_tmux_executable().unwrap_or_default();
    let mut access = ManagedCurrentRunAccessService::new(
        ManagedLaunchStore::open(config.managed_launch_store_path())?,
        ManagedCurrentRunVerifier::new(Box::new(SystemManagedCurrentRunEvidence::new(config))),
        Box::new(MacosManagedTerminalAccess::new(tmux_executable)),
    );
    run_attach_from_args_with_access(args, &mut access)
}

pub fn discover_tmux_executable() -> Option<PathBuf> {
    configured_path(["MUDI_TMUX_EXECUTABLE", "SESSIONMESH_TMUX_EXECUTABLE"])
        .filter(|path| validate_executable(path, "tmux").is_ok())
        .or_else(|| {
            std::env::var_os("PATH")
                .as_deref()
                .map(std::env::split_paths)
                .into_iter()
                .flatten()
                .map(|directory| directory.join("tmux"))
                .find(|path| validate_executable(path, "tmux").is_ok())
        })
}

fn terminate_private_tmux(
    tmux_executable: &Path,
    environment: &PrivateTmuxEnvironment,
) -> Result<()> {
    validate_executable(tmux_executable, "tmux")?;
    validate_tmux_name(&environment.socket_name)?;
    validate_tmux_name(&environment.session_name)?;
    let status = Command::new(tmux_executable)
        .env_remove("TMUX")
        .args([OsStr::new("-f"), OsStr::new("/dev/null"), OsStr::new("-L")])
        .arg(&environment.socket_name)
        .arg("kill-server")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("could not clean private tmux")?;
    if status.success() || status.code() == Some(1) {
        Ok(())
    } else {
        Err(anyhow!("private tmux cleanup failed"))
    }
}

fn clean_empty_private_tmux(
    tmux_executable: &Path,
    environment: &PrivateTmuxEnvironment,
) -> Result<()> {
    validate_executable(tmux_executable, "tmux")?;
    validate_tmux_name(&environment.socket_name)?;
    validate_tmux_name(&environment.session_name)?;
    let probe = Command::new(tmux_executable)
        .env_remove("TMUX")
        .args([OsStr::new("-f"), OsStr::new("/dev/null"), OsStr::new("-L")])
        .arg(&environment.socket_name)
        .args([OsStr::new("has-session"), OsStr::new("-t")])
        .arg(&environment.session_name)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("could not inspect private tmux")?;
    if probe.code() == Some(1) {
        Ok(())
    } else if probe.success() {
        Err(anyhow!("private tmux environment is not empty"))
    } else {
        Err(anyhow!("private tmux environment could not be inspected"))
    }
}

fn escape_tmux_format(value: &str) -> String {
    value.replace('#', "##")
}

fn tmux_window_title(title: &str, description: Option<&str>) -> String {
    escape_tmux_format(description.unwrap_or(title))
}

fn validate_executable(path: &Path, name: &str) -> Result<()> {
    let metadata =
        std::fs::metadata(path).with_context(|| format!("{name} executable is unavailable"))?;
    if !path.is_absolute() || !metadata.is_file() {
        return Err(anyhow!("{name} executable is unavailable"));
    }
    #[cfg(unix)]
    if metadata.permissions().mode() & 0o111 == 0 {
        return Err(anyhow!("{name} executable is unavailable"));
    }
    Ok(())
}

fn validate_tmux_name(value: &str) -> Result<()> {
    if value.is_empty()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return Err(anyhow!("private tmux environment is invalid"));
    }
    Ok(())
}

fn validate_tmux_pane_id(value: &str) -> Result<()> {
    if value.strip_prefix('%').is_some_and(|number| {
        !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit())
    }) {
        Ok(())
    } else {
        Err(anyhow!("private tmux pane identity is invalid"))
    }
}

fn shell_quote_os(value: &OsStr) -> Result<String> {
    value
        .to_str()
        .map(shell_quote)
        .ok_or_else(|| anyhow!("tmux executable path is not valid UTF-8"))
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

#[cfg(test)]
mod tests {
    use super::tmux_window_title;

    #[test]
    fn tmux_window_title_falls_back_to_the_session_title() {
        assert_eq!(tmux_window_title("Title #S", None), "Title ##S".to_string());
    }
}
