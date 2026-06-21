//! Render UFO2's config/ufo/agents.yaml from a vended credential.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

use crate::controlplane::Credential;

const MANAGED_USE_MCP_LINE: &str =
    "USE_MCP: False  # Managed by ufoagent: use UI automation; UFO MCP can crash GUI tasks.";
const MANAGED_MCP_YAML: &str = r#"# Managed by ufoagent - do not edit by hand.
# Keep UFO's local UI data/action servers plus HostAgent's CLI launcher. The managed
# runner still needs local MCP for screenshots, window enumeration, clicks, typing, and opening GUI
# apps/running commands. Avoid Office/HTTP MCP servers; prompt-level MCP tool loading is skipped
# separately when USE_MCP=False.
HostAgent:
  default:
    data_collection:
      - namespace: UICollector
        type: local
        start_args: []
        reset: false
    action:
      - namespace: HostUIExecutor
        type: local
        start_args: []
        reset: false
      - namespace: CommandLineExecutor
        type: local
        start_args: []
        reset: false

AppAgent:
  default:
    data_collection:
      - namespace: UICollector
        type: local
        start_args: []
        reset: false
    action:
      - namespace: AppUIExecutor
        type: local
        start_args: []
        reset: false
"#;
const MANAGED_SKIP_MCP_MARKER: &str = "Managed by ufoagent: honor USE_MCP=False";
const MCP_LOAD_SENTINEL: &str = r#"        self.logger.info("Loading MCP tool information...")"#;
const MANAGED_SKIP_MCP_GUARD: &str = r#"        # Managed by ufoagent: honor USE_MCP=False before UFO's unconditional list_tools call.
        if not get_ufo_config().system.use_mcp:
            self.logger.info("Skipping MCP tool information because USE_MCP is disabled.")
            self.prompter.create_api_prompt_template(tools=[])
            return

        self.logger.info("Loading MCP tool information...")"#;
const MANAGED_APP_CONFIRMATION_MARKER: &str =
    "Managed by ufoagent: read confirmation details from processing_context.";
const APP_CONFIRMATION_SENTINEL: &str = r#"        action = self.processor.actions
        control_text = self.processor.control_text

        decision = interactor.sensitive_step_asker(action, control_text)"#;
const MANAGED_APP_CONFIRMATION_BLOCK: &str = r#"        # Managed by ufoagent: read confirmation details from processing_context.
        context = self.processor.processing_context
        action_info = context.get_local("action_info")

        if action_info:
            action = action_info.to_list_of_dicts()
            control_text = "\n".join(action_info.to_representation())
        else:
            action = context.get_local("action", [])
            control_text = context.get_local("action_representation", "")
            if isinstance(control_text, list):
                control_text = "\n".join(control_text)

        # Managed by ufoagent: unattended GUI tasks must not prompt on stdin; overseer is the safety layer.
        decision = True"#;
#[cfg(test)]
const MANAGED_CLI_POLICY_MARKER: &str =
    "Managed by ufoagent: permit every CLI command; overseer is the safety layer.";
const MANAGED_CLI_ALLOWLIST_BLOCK: &str = r#"# Managed by ufoagent: unused; CLI policy below permits every command.
ALLOWED_CLI_COMMANDS: FrozenSet[str] = frozenset()"#;
const MANAGED_CLI_POLICY_FUNCTION: &str = r#"def _is_cli_command_allowed(command_str: str) -> bool:
    """Managed by ufoagent: permit every CLI command; overseer is the safety layer."""
    return True"#;
const UFOAGENT_WORKER: &str = include_str!("ufoagent_worker.py");
const MANAGED_RESUME_MARKER: &str =
    "Managed by ufoagent: queued resume requests for warm worker chains.";
const BASE_SESSION_INIT_SENTINEL: &str = r#"        self._rounds: Dict[int, BaseRound] = {}"#;
const BASE_SESSION_INIT_PATCH: &str = r#"        self._rounds: Dict[int, BaseRound] = {}

        # Managed by ufoagent: queued resume requests for warm worker chains.
        self._ufoagent_injected_requests = []
        self._ufoagent_resume_round_in_progress = False"#;
const BASE_SESSION_METHOD_SENTINEL: &str = r#"    @abstractmethod
    def request_to_evaluate(self) -> str:"#;
const BASE_SESSION_METHOD_PATCH: &str = r#"    # Managed by ufoagent: queued resume requests for warm worker chains.
    def ufoagent_inject_request(self, request: str) -> bool:
        request = (request or "").strip()
        if not request:
            return False
        if not hasattr(self, "_ufoagent_injected_requests"):
            self._ufoagent_injected_requests = []
        self._ufoagent_injected_requests.append(request)
        self._finish = False
        return True

    def ufoagent_has_injected_request(self) -> bool:
        return bool(getattr(self, "_ufoagent_injected_requests", []))

    def ufoagent_next_injected_request(self) -> str:
        requests = getattr(self, "_ufoagent_injected_requests", [])
        if not requests:
            return ""
        self._ufoagent_resume_round_in_progress = True
        self._finish = False
        return requests.pop(0)

    @abstractmethod
    def request_to_evaluate(self) -> str:"#;
const BASE_SESSION_IS_FINISHED_SENTINEL: &str = r#"        if (
            self._finish
            or self.step >= ufo_config.system.max_step
            or self.total_rounds >= ufo_config.system.max_round
        ):
            return True"#;
const BASE_SESSION_IS_FINISHED_PATCH: &str = r#"        # Managed by ufoagent: queued resume requests may add correction rounds
        # beyond UFO's configured max_round while preserving the same Session context.
        has_resume = self.ufoagent_has_injected_request()
        resume_round = bool(getattr(self, "_ufoagent_resume_round_in_progress", False))
        if self._finish and not has_resume and not resume_round:
            return True
        if self.step >= ufo_config.system.max_step:
            return True
        if self.total_rounds >= ufo_config.system.max_round and not has_resume and not resume_round:
            return True"#;
const SESSION_NEXT_REQUEST_SENTINEL: &str = r#"        else:
            request, iscomplete = interactor.new_request()
            if iscomplete:
                self._finish = True
            return request"#;
const SESSION_NEXT_REQUEST_PATCH: &str = r#"        else:
            injected = self.ufoagent_next_injected_request()
            if injected:
                return injected
            request, iscomplete = interactor.new_request()
            if iscomplete:
                self._finish = True
            return request"#;
const SESSION_ADD_ROUND_SENTINEL: &str = r#"        self.add_round(round.id, round)

        return round"#;
const SESSION_ADD_ROUND_PATCH: &str = r#"        self.add_round(round.id, round)
        if getattr(self, "_ufoagent_resume_round_in_progress", False):
            self._ufoagent_resume_round_in_progress = False

        return round"#;

fn yaml_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn agent_section(c: &Credential) -> String {
    format!(
        "  VISUAL_MODE: true\n  SAFE_GUARD: False\n  API_TYPE: \"openai\"\n  API_BASE: \"{}\"\n  API_KEY: \"{}\"\n  API_MODEL: \"{}\"\n",
        yaml_escape(&c.base_url),
        yaml_escape(&c.api_key),
        yaml_escape(&c.model),
    )
}

pub fn render(c: &Credential) -> String {
    let section = agent_section(c);
    // Every UFO2 agent role uses the one vended credential. Roles we leave unconfigured — notably
    // EVALUATION_AGENT — fall back to UFO2's Azure-AD default and crash *after* the task completes
    // (see issue #8), which recorded successful run_tasks as "failed". Configure all of them.
    ["HOST_AGENT", "APP_AGENT", "EVALUATION_AGENT", "BACKUP_AGENT"]
        .iter()
        .fold(
            "# Managed by ufoagent — do not edit by hand.\n# Short-lived credential, refreshed by the agent.\n".to_string(),
            |mut acc, name| {
                acc.push_str(name);
                acc.push_str(":\n");
                acc.push_str(&section);
                acc
            },
        )
}

pub fn agents_yaml_path(ufo_home: &Path) -> PathBuf {
    ufo_home.join("config").join("ufo").join("agents.yaml")
}

pub fn system_yaml_path(ufo_home: &Path) -> PathBuf {
    ufo_home.join("config").join("ufo").join("system.yaml")
}

pub fn mcp_yaml_path(ufo_home: &Path) -> PathBuf {
    ufo_home.join("config").join("ufo").join("mcp.yaml")
}

pub fn worker_path(ufo_home: &Path) -> PathBuf {
    ufo_home.join("ufoagent_worker.py")
}

pub fn install_worker(ufo_home: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(ufo_home)
        .with_context(|| format!("creating UFO home {}", ufo_home.display()))?;
    let path = worker_path(ufo_home);
    match std::fs::read_to_string(&path) {
        Ok(current) if current == UFOAGENT_WORKER => {}
        _ => std::fs::write(&path, UFOAGENT_WORKER)
            .with_context(|| format!("writing managed UFO worker {}", path.display()))?,
    }
    Ok(path)
}

fn apply_managed_mcp_config(ufo_home: &Path) -> Result<PathBuf> {
    let path = mcp_yaml_path(ufo_home);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    match std::fs::read_to_string(&path) {
        Ok(current) if current == MANAGED_MCP_YAML => {}
        _ => std::fs::write(&path, MANAGED_MCP_YAML)?,
    }
    Ok(path)
}

fn patch_mcp_loader(path: &Path) -> Result<bool> {
    let original = std::fs::read_to_string(path)
        .with_context(|| format!("reading UFO MCP loader {}", path.display()))?;
    if original.contains(MANAGED_SKIP_MCP_MARKER) {
        return Ok(false);
    }
    let patched = original.replacen(MCP_LOAD_SENTINEL, MANAGED_SKIP_MCP_GUARD, 1);
    if patched == original {
        anyhow::bail!(
            "could not find MCP loader sentinel in UFO file {}",
            path.display()
        );
    }
    std::fs::write(path, patched)
        .with_context(|| format!("writing managed UFO MCP loader patch {}", path.display()))?;
    Ok(true)
}

fn patch_app_confirmation(path: &Path) -> Result<bool> {
    let original = std::fs::read_to_string(path).with_context(|| {
        format!(
            "reading UFO AppAgent confirmation handler {}",
            path.display()
        )
    })?;
    if original.contains(MANAGED_APP_CONFIRMATION_MARKER) {
        return Ok(false);
    }
    if !original.contains("self.processor.actions")
        && !original.contains("self.processor.control_text")
    {
        return Ok(false);
    }
    let patched = original.replacen(APP_CONFIRMATION_SENTINEL, MANAGED_APP_CONFIRMATION_BLOCK, 1);
    if patched == original {
        anyhow::bail!(
            "could not find AppAgent confirmation sentinel in UFO file {}",
            path.display()
        );
    }
    std::fs::write(path, patched).with_context(|| {
        format!(
            "writing managed UFO AppAgent confirmation patch {}",
            path.display()
        )
    })?;
    Ok(true)
}

fn patch_cli_allowlist(path: &Path) -> Result<bool> {
    let original = std::fs::read_to_string(path)
        .with_context(|| format!("reading UFO CLI launcher {}", path.display()))?;
    let Some(mut patched) = replace_cli_command_policy(&original) else {
        anyhow::bail!(
            "could not find CLI launcher policy function in UFO file {}",
            path.display()
        );
    };
    if let Some(with_allowlist) = replace_cli_allowlist_block(&patched) {
        patched = with_allowlist;
    }
    if patched == original {
        return Ok(false);
    }
    std::fs::write(path, patched)
        .with_context(|| format!("writing managed UFO CLI launcher patch {}", path.display()))?;
    Ok(true)
}

fn patch_resume_base_session(path: &Path) -> Result<bool> {
    let original = std::fs::read_to_string(path)
        .with_context(|| format!("reading UFO session loop {}", path.display()))?;
    if original.contains(MANAGED_RESUME_MARKER) {
        return Ok(false);
    }
    let mut patched = original.replacen(BASE_SESSION_INIT_SENTINEL, BASE_SESSION_INIT_PATCH, 1);
    patched = patched.replacen(BASE_SESSION_METHOD_SENTINEL, BASE_SESSION_METHOD_PATCH, 1);
    patched = patched.replacen(
        BASE_SESSION_IS_FINISHED_SENTINEL,
        BASE_SESSION_IS_FINISHED_PATCH,
        1,
    );
    if patched == original
        || !patched.contains(BASE_SESSION_INIT_PATCH)
        || !patched.contains("def ufoagent_inject_request")
        || !patched.contains("has_resume = self.ufoagent_has_injected_request()")
    {
        anyhow::bail!(
            "could not find BaseSession resume sentinels in UFO file {}",
            path.display()
        );
    }
    std::fs::write(path, patched)
        .with_context(|| format!("writing managed UFO resume patch {}", path.display()))?;
    Ok(true)
}

fn patch_resume_windows_session(path: &Path) -> Result<bool> {
    let original = std::fs::read_to_string(path)
        .with_context(|| format!("reading UFO Windows session {}", path.display()))?;
    if original.contains("injected = self.ufoagent_next_injected_request()")
        && original.contains("_ufoagent_resume_round_in_progress")
    {
        return Ok(false);
    }
    let mut patched =
        original.replacen(SESSION_NEXT_REQUEST_SENTINEL, SESSION_NEXT_REQUEST_PATCH, 1);
    patched = patched.replacen(SESSION_ADD_ROUND_SENTINEL, SESSION_ADD_ROUND_PATCH, 1);
    if patched == original
        || !patched.contains("injected = self.ufoagent_next_injected_request()")
        || !patched.contains("_ufoagent_resume_round_in_progress")
    {
        anyhow::bail!(
            "could not find Windows Session resume sentinels in UFO file {}",
            path.display()
        );
    }
    std::fs::write(path, patched).with_context(|| {
        format!(
            "writing managed UFO Windows resume patch {}",
            path.display()
        )
    })?;
    Ok(true)
}

fn replace_cli_allowlist_block(original: &str) -> Option<String> {
    let start = original.find("ALLOWED_CLI_COMMANDS")?;
    let rest = &original[start..];
    let end = rest.find("\n)")? + 2;
    let mut patched =
        String::with_capacity(original.len() - end + MANAGED_CLI_ALLOWLIST_BLOCK.len());
    patched.push_str(&original[..start]);
    patched.push_str(MANAGED_CLI_ALLOWLIST_BLOCK);
    patched.push_str(&rest[end..]);
    Some(patched)
}

fn replace_cli_command_policy(original: &str) -> Option<String> {
    let start = original.find("def _is_cli_command_allowed(command_str: str) -> bool:")?;
    let rest = &original[start..];
    let end = rest.find("@MCPRegistry.register_factory_decorator")?;
    let mut patched =
        String::with_capacity(original.len() - end + MANAGED_CLI_POLICY_FUNCTION.len() + 2);
    patched.push_str(&original[..start]);
    patched.push_str(MANAGED_CLI_POLICY_FUNCTION);
    patched.push_str("\n\n");
    patched.push_str(&rest[end..]);
    Some(patched)
}

fn apply_managed_mcp_loader_patches(ufo_home: &Path) -> Result<()> {
    let app_agent = ufo_home
        .join("ufo")
        .join("agents")
        .join("agent")
        .join("app_agent.py");
    for rel in [
        ["ufo", "agents", "agent", "host_agent.py"],
        ["ufo", "agents", "agent", "app_agent.py"],
    ] {
        let path = rel.iter().fold(ufo_home.to_path_buf(), |p, c| p.join(c));
        if path.exists() {
            patch_mcp_loader(&path)?;
        }
    }
    if app_agent.exists() {
        patch_app_confirmation(&app_agent)?;
    }
    let cli = ["ufo", "client", "mcp", "local_servers", "cli_mcp_server.py"]
        .iter()
        .fold(ufo_home.to_path_buf(), |p, c| p.join(c));
    if cli.exists() {
        patch_cli_allowlist(&cli)?;
    }
    let basic = ["ufo", "module", "basic.py"]
        .iter()
        .fold(ufo_home.to_path_buf(), |p, c| p.join(c));
    if basic.exists() {
        patch_resume_base_session(&basic)?;
    }
    let session = ["ufo", "module", "sessions", "session.py"]
        .iter()
        .fold(ufo_home.to_path_buf(), |p, c| p.join(c));
    if session.exists() {
        patch_resume_windows_session(&session)?;
    }
    Ok(())
}

/// Apply UFOAgent-owned UFO defaults that keep the managed GUI runner stable.
///
/// UFO's local MCP servers are also its UI automation transport. Keep the local UI servers available
/// for screenshots/clicks/typing and the HostAgent CLI launcher, but remove Office/HTTP MCP servers
/// and skip prompt-level MCP tool loading while `USE_MCP` is false.
pub fn apply_managed_defaults(ufo_home: &Path) -> Result<PathBuf> {
    install_worker(ufo_home)?;
    apply_managed_mcp_config(ufo_home)?;
    apply_managed_mcp_loader_patches(ufo_home)?;

    let path = system_yaml_path(ufo_home);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let original = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e.into()),
    };

    let mut saw_use_mcp = false;
    let mut changed = false;
    let mut lines = Vec::new();

    for line in original.lines() {
        if line.trim_start().starts_with("USE_MCP:") {
            saw_use_mcp = true;
            if line != MANAGED_USE_MCP_LINE {
                changed = true;
            }
            lines.push(MANAGED_USE_MCP_LINE.to_string());
        } else {
            lines.push(line.to_string());
        }
    }

    if !saw_use_mcp {
        if !lines.is_empty() && lines.last().is_some_and(|l| !l.trim().is_empty()) {
            lines.push(String::new());
        }
        lines.push("# UFOAgent managed defaults".to_string());
        lines.push(MANAGED_USE_MCP_LINE.to_string());
        changed = true;
    }

    let mut rendered = lines.join("\n");
    rendered.push('\n');
    if changed || rendered != original {
        std::fs::write(&path, rendered)?;
    }
    Ok(path)
}

pub fn write(ufo_home: &Path, c: &Credential) -> Result<PathBuf> {
    let path = agents_yaml_path(ufo_home);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, render(c))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cred(key: &str) -> Credential {
        Credential {
            base_url: "https://api.openai.com/v1".into(),
            model: "gpt-4o".into(),
            api_key: key.into(),
            expires_at: 0,
            lease_id: "lse_x".into(),
        }
    }

    #[test]
    fn renders_all_agent_sections() {
        let y = render(&cred("sk-secret"));
        // All four roles must be present — a missing EVALUATION_AGENT/BACKUP_AGENT falls back to
        // UFO2's Azure-AD default and crashes after the task (issue #8).
        for role in [
            "HOST_AGENT:",
            "APP_AGENT:",
            "EVALUATION_AGENT:",
            "BACKUP_AGENT:",
        ] {
            assert!(y.contains(role), "missing {role}");
        }
        // The credential is set on every role (one per section).
        assert_eq!(y.matches("API_KEY: \"sk-secret\"").count(), 4);
        assert!(y.contains("API_TYPE: \"openai\""));
        assert!(y.contains("VISUAL_MODE: true"));
        assert_eq!(y.matches("SAFE_GUARD: False").count(), 4);
    }

    #[test]
    fn escapes_quotes_and_backslashes() {
        let y = render(&cred("a\"b\\c"));
        assert!(y.contains("API_KEY: \"a\\\"b\\\\c\""));
    }

    fn temp_home(name: &str) -> PathBuf {
        use std::time::{SystemTime, UNIX_EPOCH};
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!("ufoagent-{name}-{}-{nanos}", std::process::id()))
    }

    #[test]
    fn managed_defaults_disable_existing_mcp() {
        let home = temp_home("mcp-existing");
        let system = system_yaml_path(&home);
        let mcp = mcp_yaml_path(&home);
        std::fs::create_dir_all(system.parent().unwrap()).unwrap();
        std::fs::write(
            &system,
            "# UFO System Configuration\nUSE_MCP: True  # upstream default\nMAX_STEP: 50\n",
        )
        .unwrap();
        std::fs::write(
            &mcp,
            "AppAgent:\n  default:\n    data_collection:\n      - namespace: UICollector\n",
        )
        .unwrap();

        apply_managed_defaults(&home).unwrap();
        let y = std::fs::read_to_string(&system).unwrap();
        assert!(y.contains(MANAGED_USE_MCP_LINE));
        assert!(!y.contains("USE_MCP: True"));
        assert!(y.contains("MAX_STEP: 50"));
        let mcp_y = std::fs::read_to_string(&mcp).unwrap();
        assert_eq!(mcp_y, MANAGED_MCP_YAML);
        assert!(mcp_y.contains("namespace: UICollector"));
        assert!(mcp_y.contains("namespace: HostUIExecutor"));
        assert!(mcp_y.contains("namespace: AppUIExecutor"));
        assert!(mcp_y.contains("namespace: CommandLineExecutor"));
        assert!(!mcp_y.contains("http"));

        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn managed_defaults_create_system_yaml_when_missing() {
        let home = temp_home("mcp-missing");

        let path = apply_managed_defaults(&home).unwrap();
        let y = std::fs::read_to_string(&path).unwrap();
        assert!(y.contains("# UFOAgent managed defaults"));
        assert!(y.contains(MANAGED_USE_MCP_LINE));
        assert_eq!(
            std::fs::read_to_string(mcp_yaml_path(&home)).unwrap(),
            MANAGED_MCP_YAML
        );

        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn managed_defaults_patch_ufo_mcp_loaders() {
        let home = temp_home("mcp-loader");
        let host = home
            .join("ufo")
            .join("agents")
            .join("agent")
            .join("host_agent.py");
        let app = home
            .join("ufo")
            .join("agents")
            .join("agent")
            .join("app_agent.py");
        std::fs::create_dir_all(host.parent().unwrap()).unwrap();
        std::fs::write(
            &host,
            format!(
                "class Agent:\n    async def _load_mcp_context(self, context):\n{MCP_LOAD_SENTINEL}\n        result = await context.command_dispatcher.execute_commands([])\n"
            ),
        )
        .unwrap();
        std::fs::write(
            &app,
            format!(
                "class Agent:\n    async def _load_mcp_context(self, context):\n{MCP_LOAD_SENTINEL}\n        result = await context.command_dispatcher.execute_commands([])\n\n    def process_confirmation(self) -> bool:\n{APP_CONFIRMATION_SENTINEL}\n        return decision\n"
            ),
        )
        .unwrap();

        apply_managed_defaults(&home).unwrap();
        for path in [&host, &app] {
            let patched = std::fs::read_to_string(path).unwrap();
            assert!(patched.contains(MANAGED_SKIP_MCP_MARKER));
            assert!(patched.contains("if not get_ufo_config().system.use_mcp:"));
            assert!(patched.contains("self.prompter.create_api_prompt_template(tools=[])"));
            assert_eq!(patched.matches(MANAGED_SKIP_MCP_MARKER).count(), 1);
        }
        let app_patched = std::fs::read_to_string(&app).unwrap();
        assert!(app_patched.contains(MANAGED_APP_CONFIRMATION_MARKER));
        assert!(app_patched.contains("context = self.processor.processing_context"));
        assert!(app_patched.contains("action_info.to_list_of_dicts()"));
        assert!(app_patched.contains("action = context.get_local(\"action\", [])"));
        assert!(app_patched.contains("decision = True"));
        assert!(!app_patched.contains("interactor.sensitive_step_asker"));
        assert!(!app_patched.contains("input("));
        assert!(!app_patched.contains("self.processor.actions"));
        assert!(!app_patched.contains("self.processor.control_text"));

        apply_managed_defaults(&home).unwrap();
        for path in [&host, &app] {
            let patched = std::fs::read_to_string(path).unwrap();
            assert_eq!(patched.matches(MANAGED_SKIP_MCP_MARKER).count(), 1);
        }
        let app_patched = std::fs::read_to_string(&app).unwrap();
        assert_eq!(
            app_patched.matches(MANAGED_APP_CONFIRMATION_MARKER).count(),
            1
        );

        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn managed_defaults_patch_cli_launcher_policy() {
        let home = temp_home("cli-allowlist");
        let cli = home
            .join("ufo")
            .join("client")
            .join("mcp")
            .join("local_servers")
            .join("cli_mcp_server.py");
        std::fs::create_dir_all(cli.parent().unwrap()).unwrap();
        std::fs::write(
            &cli,
            r#"import re
import shlex
from typing import FrozenSet, List

ALLOWED_CLI_COMMANDS: FrozenSet[str] = frozenset(
    {
        # Windows applications
        "notepad",
        "notepad.exe",
        "msedge",
        "msedge.exe",
        # Common utilities
        "cmd.exe",
        "code.exe",
    }
)

_DANGEROUS_PATTERNS: List[re.Pattern] = [
    re.compile(r"\bcurl\b|\bwget\b", re.IGNORECASE),
]

def _is_cli_command_allowed(command_str: str) -> bool:
    if not command_str or not command_str.strip():
        return False
    tokens = shlex.split(command_str)
    base = tokens[0].strip().lower()
    if not any(base == allowed.lower() for allowed in ALLOWED_CLI_COMMANDS):
        return False
    for pattern in _DANGEROUS_PATTERNS:
        if pattern.search(command_str):
            return False
    return True

@MCPRegistry.register_factory_decorator("CommandLineExecutor")
def create_cli_mcp_server(*args, **kwargs):
    return None
"#,
        )
        .unwrap();

        apply_managed_defaults(&home).unwrap();
        let patched = std::fs::read_to_string(&cli).unwrap();
        assert!(patched.contains(MANAGED_CLI_POLICY_MARKER));
        assert!(patched.contains("ALLOWED_CLI_COMMANDS: FrozenSet[str] = frozenset()"));
        assert!(patched.contains("return True"));
        assert!(patched.contains("@MCPRegistry.register_factory_decorator"));
        assert!(
            !patched.contains("any(base == allowed.lower() for allowed in ALLOWED_CLI_COMMANDS)")
        );
        assert!(!patched.contains("pattern.search(command_str)"));
        assert!(!patched.contains("\"msedge.exe\""));
        assert!(!patched.contains("\"code.exe\""));
        assert!(!patched.contains("\"notepad.exe\""));
        assert!(!patched.contains("\"notepad-plus-plus.cmd\""));
        assert!(!patched.contains("\"notepad++.exe\""));
        assert_eq!(patched.matches(MANAGED_CLI_POLICY_MARKER).count(), 1);

        apply_managed_defaults(&home).unwrap();
        let patched = std::fs::read_to_string(&cli).unwrap();
        assert_eq!(patched.matches(MANAGED_CLI_POLICY_MARKER).count(), 1);

        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn managed_defaults_repairs_partial_managed_cli_launcher_policy() {
        let home = temp_home("cli-allowlist-migrate");
        let cli = home
            .join("ufo")
            .join("client")
            .join("mcp")
            .join("local_servers")
            .join("cli_mcp_server.py");
        std::fs::create_dir_all(cli.parent().unwrap()).unwrap();
        std::fs::write(
            &cli,
            r#"import shlex
from typing import FrozenSet

ALLOWED_CLI_COMMANDS: FrozenSet[str] = frozenset(
    {
        # Windows applications
        "msedge",
        "msedge.exe",
        "chrome",
        "chrome.exe",
        # Common utilities
        "code",
        "code.exe",
        # Managed by ufoagent: allow one constrained launcher instead of per-app shell entries.
        "ufoagent-launch",
        "ufoagent-launch.cmd",
    }
)

def _is_cli_command_allowed(command_str: str) -> bool:
    tokens = shlex.split(command_str)
    base = tokens[0].strip().lower()
    return any(base == allowed.lower() for allowed in ALLOWED_CLI_COMMANDS)

@MCPRegistry.register_factory_decorator("CommandLineExecutor")
def create_cli_mcp_server(*args, **kwargs):
    return None
"#,
        )
        .unwrap();

        apply_managed_defaults(&home).unwrap();
        let patched = std::fs::read_to_string(&cli).unwrap();
        assert!(patched.contains(MANAGED_CLI_POLICY_MARKER));
        assert!(patched.contains("ALLOWED_CLI_COMMANDS: FrozenSet[str] = frozenset()"));
        assert!(patched.contains("return True"));
        assert!(!patched.contains("allow one constrained launcher"));
        assert!(!patched.contains("\"ufoagent-launch.cmd\""));
        assert!(!patched.contains("\"msedge.exe\""));
        assert!(!patched.contains("\"chrome.exe\""));
        assert!(!patched.contains("\"code.exe\""));
        assert!(!patched.contains("\"bambu-studio.cmd\""));
        assert!(!patched.contains("\"notepad-plus-plus.cmd\""));
        assert_eq!(patched.matches(MANAGED_CLI_POLICY_MARKER).count(), 1);

        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn managed_defaults_install_worker_and_patch_resume_adapter() {
        let home = temp_home("warm-worker");
        let basic = home.join("ufo").join("module").join("basic.py");
        let session = home
            .join("ufo")
            .join("module")
            .join("sessions")
            .join("session.py");
        std::fs::create_dir_all(basic.parent().unwrap()).unwrap();
        std::fs::create_dir_all(session.parent().unwrap()).unwrap();
        std::fs::write(
            &basic,
            format!(
                r#"class BaseSession:
    def __init__(self):
{BASE_SESSION_INIT_SENTINEL}

    def is_finished(self) -> bool:
{BASE_SESSION_IS_FINISHED_SENTINEL}

{BASE_SESSION_METHOD_SENTINEL}
        pass
"#
            ),
        )
        .unwrap();
        std::fs::write(
            &session,
            format!(
                r#"class Session:
    def create_new_round(self):
        round = object()
{SESSION_ADD_ROUND_SENTINEL}

    def next_request(self) -> str:
        if self.total_rounds == 0:
            return self._init_request
{SESSION_NEXT_REQUEST_SENTINEL}
"#
            ),
        )
        .unwrap();

        apply_managed_defaults(&home).unwrap();
        assert_eq!(
            std::fs::read_to_string(worker_path(&home)).unwrap(),
            UFOAGENT_WORKER
        );
        let basic_patched = std::fs::read_to_string(&basic).unwrap();
        assert!(basic_patched.contains(MANAGED_RESUME_MARKER));
        assert!(basic_patched.contains("def ufoagent_inject_request"));
        assert!(basic_patched.contains("has_resume = self.ufoagent_has_injected_request()"));
        let session_patched = std::fs::read_to_string(&session).unwrap();
        assert!(session_patched.contains("injected = self.ufoagent_next_injected_request()"));
        assert!(session_patched.contains("_ufoagent_resume_round_in_progress"));

        apply_managed_defaults(&home).unwrap();
        let basic_patched = std::fs::read_to_string(&basic).unwrap();
        assert_eq!(basic_patched.matches(MANAGED_RESUME_MARKER).count(), 2);

        let _ = std::fs::remove_dir_all(home);
    }
}
