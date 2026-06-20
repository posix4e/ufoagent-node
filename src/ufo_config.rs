//! Render UFO2's config/ufo/agents.yaml from a vended credential.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

use crate::controlplane::Credential;

const MANAGED_USE_MCP_LINE: &str =
    "USE_MCP: False  # Managed by ufoagent: use UI automation; UFO MCP can crash GUI tasks.";
const MANAGED_MCP_YAML: &str = r#"# Managed by ufoagent - do not edit by hand.
# Keep UFO's local UI data/action servers plus HostAgent's generic command executor. The managed
# runner still needs local MCP for screenshots, window enumeration, clicks, typing, and opening GUI
# apps. Avoid Office/HTTP MCP servers; prompt-level MCP tool loading is skipped separately when
# USE_MCP=False.
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
      - namespace: CommandLineExecutor
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
    "Managed by ufoagent: auto-approve confirmation in unattended mode.";
const APP_CONFIRMATION_SENTINEL: &str = r#"        action = self.processor.actions
        control_text = self.processor.control_text

        decision = interactor.sensitive_step_asker(action, control_text)"#;
const MANAGED_APP_CONFIRMATION_BLOCK: &str = r#"        # Managed by ufoagent: auto-approve confirmation in unattended mode.
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

        self.logger.info("Auto-approving UFO sensitive action confirmation: %s", control_text)
        decision = True"#;
const MANAGED_SHELL_COMMAND_MARKER: &str =
    "Managed by ufoagent: allow unrestricted run_shell commands.";
const MANAGED_SHELL_COMMAND_BLOCK: &str = r#"def _is_command_allowed(command_str: str) -> bool:
    """Managed by ufoagent: allow unrestricted run_shell commands."""
    return bool(command_str and command_str.strip())

"#;
const MANAGED_SHELL_PATH_MARKER: &str = "Managed by ufoagent: allow unrestricted run_shell paths.";
const MANAGED_SHELL_PATH_BLOCK: &str = r#"def _validate_command_paths(command_str: str, base_directory: str) -> Optional[str]:
    """Managed by ufoagent: allow unrestricted run_shell paths."""
    return None

"#;
const MANAGED_CLI_COMMAND_MARKER: &str =
    "Managed by ufoagent: allow unrestricted CLI launcher commands.";
const MANAGED_CLI_COMMAND_BLOCK: &str = r#"def _is_cli_command_allowed(command_str: str) -> bool:
    """Managed by ufoagent: allow unrestricted CLI launcher commands."""
    return bool(command_str and command_str.strip())

"#;
const MANAGED_CLI_RUN_SHELL_MARKER: &str =
    "Managed by ufoagent: launch unrestricted commands without waiting for GUI apps.";
const CLI_RUN_SHELL_SENTINEL: &str = r#"    @cli_mcp.tool()
    def run_shell(
        bash_command: str,
    ) -> None:
        """
        Launch an application using the provided command.
        Only allow-listed applications may be launched.
        :param bash_command: The command to execute to launch the application.
        :return: None
        """

        if not bash_command:
            raise ToolError("Bash command cannot be empty.")

        if not _is_cli_command_allowed(bash_command):
            raise ToolError(
                "Command blocked by security policy. "
                "Only allow-listed applications may be launched."
            )

        try:
            # Parse into argument list and launch without shell=True
            # to prevent shell injection.
            args = shlex.split(bash_command)
            subprocess.Popen(args, shell=False)
            time.sleep(5)  # Wait for the application to launch
        except Exception as e:
            raise ToolError(f"Failed to launch application: {str(e)}")
"#;
const MANAGED_CLI_RUN_SHELL_BLOCK: &str = r#"    @cli_mcp.tool()
    def run_shell(
        bash_command: str,
    ) -> dict:
        """
        Managed by ufoagent: launch unrestricted commands without waiting for GUI apps.
        :param bash_command: The command to execute.
        :return: started, pid, and command.
        """

        if not bash_command:
            raise ToolError("Bash command cannot be empty.")

        if not _is_cli_command_allowed(bash_command):
            raise ToolError(
                "Command blocked by security policy. "
                "Only allow-listed applications may be launched."
            )

        try:
            args = shlex.split(bash_command)
            process = subprocess.Popen(
                args,
                stdin=subprocess.DEVNULL,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                shell=False,
            )
            time.sleep(5)  # Wait for GUI applications to create their first window.
            return {
                "started": True,
                "pid": process.pid,
                "command": bash_command,
            }
        except ToolError:
            raise
        except Exception as e:
            raise ToolError(f"Command execution failed: {str(e)}")
"#;
const MANAGED_UI_KEYBOARD_INPUT_MARKER: &str =
    "Managed by ufoagent: allow focused-app keyboard input.";
const UI_KEYBOARD_INPUT_START: &str =
    "    @action_mcp.tool(tags={\"AppAgent\"}, exclude_args=[])\n    def keyboard_input(";
const MANAGED_UI_KEYBOARD_INPUT_BLOCK: &str = r#"    @action_mcp.tool(tags={"AppAgent"}, exclude_args=[])
    def keyboard_input(
        keys: Annotated[
            str,
            Field(
                description="Key sequence to send. It can be any key on the keyboard, with special keys represented by their virtual key codes, for example, '{VK_CONTROL}c' for Ctrl+C."
            ),
        ],
        id: Annotated[
            Optional[str],
            Field(
                description="Optional annotated ID of the selected control item. Managed by ufoagent: omit this to send keys to the focused application window."
            ),
        ] = None,
        name: Annotated[
            Optional[str],
            Field(
                description="Optional name of the selected control item. Managed by ufoagent: when id is provided without name, the name is resolved from the current control list."
            ),
        ] = None,
        control_focus: Annotated[
            bool,
            Field(
                description="Whether to focus the selected control id before sending keys. If False, the hotkeys will operate on the application window."
            ),
        ] = True,
    ) -> Annotated[
        str, Field(description="The key of the control item to send keyboard input to.")
    ]:
        """
        Managed by ufoagent: allow focused-app keyboard input.
        Simulate keyboard input to a control or the focused application.
        """
        normalized_keys = {
            "enter": "{ENTER}",
            "return": "{ENTER}",
            "tab": "{TAB}",
            "escape": "{ESC}",
            "esc": "{ESC}",
            "space": "{SPACE}",
            "backspace": "{BACKSPACE}",
            "delete": "{DELETE}",
        }
        if isinstance(keys, str):
            keys = normalized_keys.get(keys.strip().lower(), keys)

        target = None
        warning = None

        if id:
            if not ui_state.control_dict:
                raise ToolError(
                    "No controls available. Please refresh the application state before targeting a control."
                )
            control = ui_state.control_dict.get(id)
            if not control:
                raise ToolError(
                    f"Control with id '{id}' not found. Available control ids: {list(ui_state.control_dict.keys())}"
                )
            true_name = control.element_info.name if hasattr(control, "element_info") else ""
            if name is None:
                name = true_name
            elif true_name != name:
                warning = f"Warning: The name of your chosen control id {id} is {true_name}, but the name argument is {name}. The action is performed on control {id}:{true_name}."
            target = TargetInfo(id=id, name=name or "", kind="control")
        elif name and ui_state.control_dict:
            matches = [
                (control_id, control)
                for control_id, control in ui_state.control_dict.items()
                if hasattr(control, "element_info")
                and control.element_info.name == name
            ]
            if len(matches) == 1:
                id, control = matches[0]
                target = TargetInfo(id=id, name=name, kind="control")
            else:
                control_focus = False
        else:
            control_focus = False

        action = ActionCommandInfo(
            function="keyboard_input",
            arguments={"keys": keys, "control_focus": control_focus},
            target=target,
        )

        result = _execute_action(action)

        if warning:
            return f"{warning} {result}"
        return result

"#;
const MANAGED_RUNTIME_REMEDIATION_MARKER: &str =
    "Managed by ufoagent: generic runtime remediation.";
const MANAGED_RUNTIME_REMEDIATION_LINE: &str = "  - Managed by ufoagent: generic runtime remediation. If an action fails because a required runtime, library, driver, system capability, or executable is missing, diagnose the visible error, use available tools to install or configure the missing component or a software fallback, retry the original action, and verify the application runs without the error before finishing. When using shell tools or package managers, use noninteractive flags when available and treat non-zero exit codes, prompts, or error output as failures to resolve before continuing.";

fn yaml_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn agent_section(c: &Credential) -> String {
    format!(
        "  VISUAL_MODE: true\n  API_TYPE: \"openai\"\n  API_BASE: \"{}\"\n  API_KEY: \"{}\"\n  API_MODEL: \"{}\"\n",
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

fn patch_python_function(
    path: &Path,
    function_name: &str,
    replacement: &str,
    marker: &str,
) -> Result<bool> {
    let original = std::fs::read_to_string(path)
        .with_context(|| format!("reading UFO Python source {}", path.display()))?;
    if original.contains(marker) {
        return Ok(false);
    }
    let Some(patched) = replace_python_function(&original, function_name, replacement) else {
        anyhow::bail!(
            "could not find Python function {function_name} in UFO file {}",
            path.display()
        );
    };
    if patched == original {
        return Ok(false);
    }
    std::fs::write(path, patched)
        .with_context(|| format!("writing managed UFO Python patch {}", path.display()))?;
    Ok(true)
}

fn replace_python_function(
    original: &str,
    function_name: &str,
    replacement: &str,
) -> Option<String> {
    let needle = format!("def {function_name}(");
    let start = original.find(&needle)?;
    let rest = &original[start..];
    let mut end = rest.len();
    let mut offset = 0;
    for line in rest.split_inclusive('\n') {
        if offset > 0
            && (line.starts_with("def ") || line.starts_with("class ") || line.starts_with('@'))
        {
            end = offset;
            break;
        }
        offset += line.len();
    }
    let mut patched = String::with_capacity(original.len() - end + replacement.len());
    patched.push_str(&original[..start]);
    patched.push_str(replacement);
    patched.push_str(&rest[end..]);
    Some(patched)
}

fn patch_shell_command_executor(path: &Path) -> Result<()> {
    patch_python_function(
        path,
        "_is_command_allowed",
        MANAGED_SHELL_COMMAND_BLOCK,
        MANAGED_SHELL_COMMAND_MARKER,
    )?;
    patch_python_function(
        path,
        "_validate_command_paths",
        MANAGED_SHELL_PATH_BLOCK,
        MANAGED_SHELL_PATH_MARKER,
    )?;
    Ok(())
}

fn patch_cli_command_executor(path: &Path) -> Result<bool> {
    let mut changed = patch_python_function(
        path,
        "_is_cli_command_allowed",
        MANAGED_CLI_COMMAND_BLOCK,
        MANAGED_CLI_COMMAND_MARKER,
    )?;
    let original = std::fs::read_to_string(path)
        .with_context(|| format!("reading UFO CLI MCP server {}", path.display()))?;
    if !original.contains(MANAGED_CLI_RUN_SHELL_MARKER) {
        let patched = if original.contains(CLI_RUN_SHELL_SENTINEL) {
            original.replacen(CLI_RUN_SHELL_SENTINEL, MANAGED_CLI_RUN_SHELL_BLOCK, 1)
        } else {
            replace_cli_run_shell_block(&original, MANAGED_CLI_RUN_SHELL_BLOCK).ok_or_else(
                || {
                    anyhow::anyhow!(
                        "could not find CommandLineExecutor run_shell block in UFO file {}",
                        path.display()
                    )
                },
            )?
        };
        if patched == original {
            anyhow::bail!(
                "could not find CommandLineExecutor run_shell sentinel in UFO file {}",
                path.display()
            );
        }
        std::fs::write(path, patched).with_context(|| {
            format!("writing managed UFO CLI run_shell patch {}", path.display())
        })?;
        changed = true;
    }
    Ok(changed)
}

fn replace_cli_run_shell_block(original: &str, replacement: &str) -> Option<String> {
    let start = original.find("    @cli_mcp.tool()\n    def run_shell(")?;
    let end_marker = "\n    return cli_mcp";
    let end = start + original[start..].find(end_marker)?;
    let mut patched = String::with_capacity(original.len() - (end - start) + replacement.len());
    patched.push_str(&original[..start]);
    patched.push_str(replacement);
    patched.push_str(&original[end..]);
    Some(patched)
}

fn patch_ui_keyboard_input(path: &Path) -> Result<bool> {
    let original = std::fs::read_to_string(path)
        .with_context(|| format!("reading UFO UI MCP server {}", path.display()))?;
    if original.contains(MANAGED_UI_KEYBOARD_INPUT_MARKER) {
        return Ok(false);
    }
    let Some(patched) = replace_ui_keyboard_input_block(&original, MANAGED_UI_KEYBOARD_INPUT_BLOCK)
    else {
        anyhow::bail!(
            "could not find AppUIExecutor keyboard_input block in UFO file {}",
            path.display()
        );
    };
    if patched == original {
        return Ok(false);
    }
    std::fs::write(path, patched)
        .with_context(|| format!("writing managed UFO UI keyboard patch {}", path.display()))?;
    Ok(true)
}

fn replace_ui_keyboard_input_block(original: &str, replacement: &str) -> Option<String> {
    let start = original.find(UI_KEYBOARD_INPUT_START)?;
    let rest_after_start = &original[start + UI_KEYBOARD_INPUT_START.len()..];
    let end = if let Some(next_tool) = rest_after_start.find("\n    @action_mcp.tool(") {
        start + UI_KEYBOARD_INPUT_START.len() + next_tool
    } else {
        start + original[start..].find("\n    return action_mcp")?
    };
    let mut patched = String::with_capacity(original.len() - (end - start) + replacement.len());
    patched.push_str(&original[..start]);
    patched.push_str(replacement);
    patched.push_str(&original[end..]);
    Some(patched)
}

fn patch_runtime_remediation_prompt(path: &Path, heading: &str) -> Result<bool> {
    let original = std::fs::read_to_string(path)
        .with_context(|| format!("reading UFO prompt {}", path.display()))?;
    if original.contains(MANAGED_RUNTIME_REMEDIATION_MARKER) {
        return Ok(false);
    }
    let sentinel = format!("  {heading}\n");
    if !original.contains(&sentinel) {
        anyhow::bail!(
            "could not find prompt heading {heading} in UFO file {}",
            path.display()
        );
    }
    let insertion = format!("{sentinel}{MANAGED_RUNTIME_REMEDIATION_LINE}\n");
    let patched = original.replacen(&sentinel, &insertion, 1);
    std::fs::write(path, patched)
        .with_context(|| format!("writing managed UFO prompt patch {}", path.display()))?;
    Ok(true)
}

fn apply_managed_ufo_source_patches(ufo_home: &Path) -> Result<()> {
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
    let shell = ["ufo", "automator", "app_apis", "shell", "shell_client.py"]
        .iter()
        .fold(ufo_home.to_path_buf(), |p, c| p.join(c));
    if shell.exists() {
        patch_shell_command_executor(&shell)?;
    }
    let cli = ["ufo", "client", "mcp", "local_servers", "cli_mcp_server.py"]
        .iter()
        .fold(ufo_home.to_path_buf(), |p, c| p.join(c));
    if cli.exists() {
        patch_cli_command_executor(&cli)?;
    }
    let ui = ["ufo", "client", "mcp", "local_servers", "ui_mcp_server.py"]
        .iter()
        .fold(ufo_home.to_path_buf(), |p, c| p.join(c));
    if ui.exists() {
        patch_ui_keyboard_input(&ui)?;
    }
    for (rel, heading) in [
        (
            ["ufo", "prompts", "share", "base", "host_agent.yaml"],
            "## Guidelines",
        ),
        (
            ["ufo", "prompts", "share", "base", "app_agent.yaml"],
            "## Other Guidelines",
        ),
    ] {
        let path = rel.iter().fold(ufo_home.to_path_buf(), |p, c| p.join(c));
        if path.exists() {
            patch_runtime_remediation_prompt(&path, heading)?;
        }
    }
    Ok(())
}

/// Apply UFOAgent-owned UFO defaults that keep the managed GUI runner stable.
///
/// UFO's local MCP servers are also its UI automation transport. Keep the local UI servers available
/// for screenshots/clicks/typing and the generic HostAgent command executor, but remove Office/HTTP
/// MCP servers and skip prompt-level MCP tool loading while `USE_MCP` is false.
pub fn apply_managed_defaults(ufo_home: &Path) -> Result<PathBuf> {
    apply_managed_mcp_config(ufo_home)?;
    apply_managed_ufo_source_patches(ufo_home)?;

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
        assert_eq!(mcp_y.matches("namespace: CommandLineExecutor").count(), 2);
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
        let shell = home
            .join("ufo")
            .join("automator")
            .join("app_apis")
            .join("shell")
            .join("shell_client.py");
        let ui = home
            .join("ufo")
            .join("client")
            .join("mcp")
            .join("local_servers")
            .join("ui_mcp_server.py");
        let cli = home
            .join("ufo")
            .join("client")
            .join("mcp")
            .join("local_servers")
            .join("cli_mcp_server.py");
        let host_prompt = home
            .join("ufo")
            .join("prompts")
            .join("share")
            .join("base")
            .join("host_agent.yaml");
        let app_prompt = home
            .join("ufo")
            .join("prompts")
            .join("share")
            .join("base")
            .join("app_agent.yaml");
        std::fs::create_dir_all(host.parent().unwrap()).unwrap();
        std::fs::create_dir_all(shell.parent().unwrap()).unwrap();
        std::fs::create_dir_all(ui.parent().unwrap()).unwrap();
        std::fs::create_dir_all(cli.parent().unwrap()).unwrap();
        std::fs::create_dir_all(host_prompt.parent().unwrap()).unwrap();
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
        std::fs::write(
            &shell,
            r#"from typing import Optional

def _is_command_allowed(command_str: str) -> bool:
    if not command_str:
        return False
    return False

def _validate_command_paths(command_str: str, base_directory: str) -> Optional[str]:
    if not command_str:
        return None
    return "blocked"

class ShellReceiver:
    pass
"#,
        )
        .unwrap();
        std::fs::write(
            &ui,
            r#"from typing import Annotated, Optional
from pydantic import Field

def create_app_action_mcp_server(*args, **kwargs):
    action_mcp = FastMCP("UFO UI AppAgent Action MCP Server")

    @action_mcp.tool(tags={"AppAgent"}, exclude_args=[])
    def set_edit_text(id, name, text, clear_current_text=False):
        pass

    @action_mcp.tool(tags={"AppAgent"}, exclude_args=[])
    def keyboard_input(
        id: Annotated[
            str,
            Field(
                description="The precise annotated ID of the selected control item to send keyboard input to, adhering strictly to the provided options in the field of 'id' in the control information."
            ),
        ],
        name: Annotated[
            str,
            Field(
                description="The precise name of the selected control item to send keyboard input to, adhering strictly to the provided options in the field of 'name' in the control information."
            ),
        ],
        keys: Annotated[
            str,
            Field(
                description="Key sequence to send. It can be any key on the keyboard, with special keys represented by their virtual key codes, for example, '{VK_CONTROL}c' for Ctrl+C."
            ),
        ],
        control_focus: Annotated[
            bool,
            Field(
                description="Whether to focus the selected control id before sending keys. If False, the hotkeys will operate on the application window."
            ),
        ] = True,
    ) -> Annotated[
        str, Field(description="The key of the control item to send keyboard input to.")
    ]:
        """
        Simulate keyboard input to a control or the focused application, such as sending key presses or shortcuts.
        """
        action = ActionCommandInfo(
            function="keyboard_input",
            arguments={"keys": keys, "control_focus": control_focus},
            target=TargetInfo(id=id, name=name, kind="control"),
        )

        return _execute_action(action)

    @action_mcp.tool(tags={"AppAgent"}, exclude_args=[])
    def wheel_mouse_input(id, name, wheel_dist):
        pass

    return action_mcp
"#,
        )
        .unwrap();
        std::fs::write(
            &cli,
            r#"import shlex
import subprocess
import time

def _is_cli_command_allowed(command_str: str) -> bool:
    if not command_str:
        return False
    return False

@MCPRegistry.register_factory_decorator("CommandLineExecutor")
def create_cli_mcp_server(*args, **kwargs):
    cli_mcp = FastMCP("UFO CLI MCP Server")

    @cli_mcp.tool()
    def run_shell(
        bash_command: str,
    ) -> None:
        """
        Launch an application using the provided command.
        Only allow-listed applications may be launched.
        :param bash_command: The command to execute to launch the application.
        :return: None
        """

        if not bash_command:
            raise ToolError("Bash command cannot be empty.")

        if not _is_cli_command_allowed(bash_command):
            raise ToolError(
                "Command blocked by security policy. "
                "Only allow-listed applications may be launched."
            )

        try:
            # Parse into argument list and launch without shell=True
            # to prevent shell injection.
            args = shlex.split(bash_command)
            subprocess.Popen(args, shell=False)
            time.sleep(5)  # Wait for the application to launch
        except Exception as e:
            raise ToolError(f"Failed to launch application: {str(e)}")

    return cli_mcp
"#,
        )
        .unwrap();
        std::fs::write(
            &host_prompt,
            "system: |-\n  ## Guidelines\n  - Existing host instruction.\n\nsystem_nonvisual: |-\n  ## Guidelines\n  - Existing nonvisual host instruction.\n",
        )
        .unwrap();
        std::fs::write(
            &app_prompt,
            "system: |-\n  ## Other Guidelines\n  - Existing app instruction.\n\nsystem_as: |-\n  ## Other Guidelines\n  - Existing app-as instruction.\n",
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
        assert!(app_patched.contains("Auto-approving UFO sensitive action confirmation"));
        assert!(!app_patched.contains("sensitive_step_asker(action, control_text)"));
        assert!(!app_patched.contains("self.processor.actions"));
        assert!(!app_patched.contains("self.processor.control_text"));
        let shell_patched = std::fs::read_to_string(&shell).unwrap();
        assert!(shell_patched.contains(MANAGED_SHELL_COMMAND_MARKER));
        assert!(shell_patched.contains(MANAGED_SHELL_PATH_MARKER));
        assert!(shell_patched.contains("return bool(command_str and command_str.strip())"));
        assert!(shell_patched.contains("return None"));
        assert!(!shell_patched.contains("return \"blocked\""));
        let cli_patched = std::fs::read_to_string(&cli).unwrap();
        assert!(cli_patched.contains(MANAGED_CLI_COMMAND_MARKER));
        assert!(cli_patched.contains(MANAGED_CLI_RUN_SHELL_MARKER));
        assert!(cli_patched.contains("return bool(command_str and command_str.strip())"));
        assert!(cli_patched.contains("args = shlex.split(bash_command)"));
        assert!(cli_patched.contains("process = subprocess.Popen("));
        assert!(cli_patched.contains("stdout=subprocess.DEVNULL"));
        assert!(cli_patched.contains("stderr=subprocess.DEVNULL"));
        assert!(cli_patched.contains("stdin=subprocess.DEVNULL"));
        assert!(cli_patched.contains("time.sleep(5)"));
        assert!(cli_patched.contains("\"started\": True"));
        assert!(cli_patched.contains("\"pid\": process.pid"));
        assert!(cli_patched.contains("@MCPRegistry.register_factory_decorator"));
        assert!(!cli_patched.contains("timeout=600"));
        assert!(!cli_patched.contains("subprocess.run("));
        assert!(!cli_patched.contains("[\"cmd.exe\", \"/d\", \"/s\", \"/c\", bash_command]"));
        assert!(!cli_patched.contains("subprocess.Popen(args, shell=False)"));
        let ui_patched = std::fs::read_to_string(&ui).unwrap();
        assert!(ui_patched.contains(MANAGED_UI_KEYBOARD_INPUT_MARKER));
        assert!(ui_patched.contains("keys: Annotated["));
        assert!(ui_patched.contains("id: Annotated["));
        assert!(ui_patched.contains("Optional[str]"));
        assert!(ui_patched.contains("\"enter\": \"{ENTER}\""));
        assert!(ui_patched.contains("control_focus = False"));
        assert!(ui_patched.contains("target=target"));
        assert!(!ui_patched.contains("target=TargetInfo(id=id, name=name, kind=\"control\")"));
        for path in [&host_prompt, &app_prompt] {
            let prompt = std::fs::read_to_string(path).unwrap();
            assert!(prompt.contains(MANAGED_RUNTIME_REMEDIATION_MARKER));
            assert!(prompt.contains("diagnose the visible error"));
            assert!(prompt.contains("software fallback"));
            assert!(prompt.contains("noninteractive flags"));
        }

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
        let shell_patched = std::fs::read_to_string(&shell).unwrap();
        assert_eq!(
            shell_patched.matches(MANAGED_SHELL_COMMAND_MARKER).count(),
            1
        );
        assert_eq!(shell_patched.matches(MANAGED_SHELL_PATH_MARKER).count(), 1);
        let cli_patched = std::fs::read_to_string(&cli).unwrap();
        assert_eq!(cli_patched.matches(MANAGED_CLI_COMMAND_MARKER).count(), 1);
        assert_eq!(cli_patched.matches(MANAGED_CLI_RUN_SHELL_MARKER).count(), 1);
        let ui_patched = std::fs::read_to_string(&ui).unwrap();
        assert_eq!(
            ui_patched.matches(MANAGED_UI_KEYBOARD_INPUT_MARKER).count(),
            1
        );
        for path in [&host_prompt, &app_prompt] {
            let prompt = std::fs::read_to_string(path).unwrap();
            assert_eq!(
                prompt.matches(MANAGED_RUNTIME_REMEDIATION_MARKER).count(),
                1
            );
        }

        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn cli_launcher_patch_replaces_previous_managed_block() {
        let original = r#"def create_cli_mcp_server(*args, **kwargs):
    cli_mcp = FastMCP("UFO CLI MCP Server")

    @cli_mcp.tool()
    def run_shell(
        bash_command: str,
    ) -> dict:
        """Managed by ufoagent: execute unrestricted commands through cmd.exe."""
        completed = subprocess.run(
            ["cmd.exe", "/d", "/s", "/c", bash_command],
            timeout=600,
        )
        return {"return_code": completed.returncode}

    return cli_mcp
"#;

        let patched = replace_cli_run_shell_block(original, MANAGED_CLI_RUN_SHELL_BLOCK).unwrap();

        assert!(patched.contains(MANAGED_CLI_RUN_SHELL_MARKER));
        assert!(patched.contains("process = subprocess.Popen("));
        assert!(patched.contains("\"started\": True"));
        assert!(!patched.contains("execute unrestricted commands through cmd.exe"));
        assert!(!patched.contains("[\"cmd.exe\", \"/d\", \"/s\", \"/c\", bash_command]"));
        assert!(!patched.contains("timeout=600"));
    }
}
