//! Render UFO2's config/ufo/agents.yaml from a vended credential.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

use crate::controlplane::Credential;

const UNATTENDED_SAFE_GUARD_LINE: &str =
    "SAFE_GUARD: False  # Managed by ufoagent: unattended runs auto-approve confirmations.";
const CLI_ALLOW_ALL_MARKER: &str =
    "Managed by ufoagent: allow all CLI MCP commands for unattended installs.";
const CLI_ALLOW_ALL_FUNCTION: &str = r#"def _is_cli_command_allowed(command_str: str) -> bool:
    """Managed by ufoagent: allow all CLI MCP commands for unattended installs."""
    return bool(command_str and command_str.strip())

"#;
const GUI_ACTIONS_MARKER: &str =
    "Managed by ufoagent: make GUI action primitives tolerant and on-screen.";
const GUI_ACTIONS_HELPERS: &str = r#"
# Managed by ufoagent: make GUI action primitives tolerant and on-screen.
def _ufoagent_restore_window_for_actions(window: Optional[UIAWrapper]) -> None:
    if not window:
        return
    try:
        if hasattr(window, "is_minimized") and window.is_minimized():
            window.restore()
            time.sleep(0.15)
    except Exception:
        pass
    try:
        window.set_focus()
        time.sleep(0.05)
    except Exception:
        pass
    try:
        rect = window.rectangle()
        unusable = (
            rect.width() < 200
            or rect.height() < 100
            or rect.right <= 0
            or rect.bottom <= 0
            or rect.left < -5000
            or rect.top < -5000
        )
        if unusable:
            try:
                window.restore()
                time.sleep(0.1)
            except Exception:
                pass
            try:
                window.move_window(x=0, y=0, width=1280, height=800, repaint=True)
                time.sleep(0.1)
            except Exception:
                pass
        try:
            window.maximize()
            time.sleep(0.1)
        except Exception:
            pass
        try:
            window.set_focus()
        except Exception:
            pass
    except Exception:
        pass


def _ufoagent_control_rect_usable(
    control: UIAWrapper, window: Optional[UIAWrapper] = None
) -> bool:
    try:
        rect = control.rectangle()
        if rect.width() <= 1 or rect.height() <= 1:
            return False
        if rect.right <= 0 or rect.bottom <= 0 or rect.left < -5000 or rect.top < -5000:
            return False
        if window:
            try:
                window_rect = window.rectangle()
                if window_rect.width() > 1 and window_rect.height() > 1:
                    if rect.right < window_rect.left - 50 or rect.left > window_rect.right + 50:
                        return False
                    if rect.bottom < window_rect.top - 50 or rect.top > window_rect.bottom + 50:
                        return False
            except Exception:
                pass
        return True
    except Exception:
        return False


def _ufoagent_current_controls(ui_state: "UIServerState") -> Dict[str, UIAWrapper]:
    if not ui_state.selected_app_window:
        return {}
    _ufoagent_restore_window_for_actions(ui_state.selected_app_window)
    controls_list = ui_state.control_inspector.find_control_elements_in_descendants(
        ui_state.selected_app_window,
        control_type_list=configs.get("CONTROL_LIST", []),
        class_name_list=configs.get("CONTROL_LIST", []),
    )
    usable_controls = [
        control
        for control in controls_list
        if _ufoagent_control_rect_usable(control, ui_state.selected_app_window)
    ]
    return {str(i + 1): control for i, control in enumerate(usable_controls)}

"#;
const SELECT_APPLICATION_WINDOW_FUNCTION: &str = r#"    def select_application_window(
        id: Annotated[
            str,
            "Specify the precise label of the application or third-party agents to be selected for the current sub-task, adhering strictly to the provided options in the field of id in the application information.",
        ],
        name: Annotated[
            str,
            "Specify the precise name of the application or third-party agents to be selected for the current sub-task, adhering strictly to the provided options and matching the selected id.",
        ],
    ) -> Dict[str, Any]:
        """
        Select an application window for UI automation.
        :return: Information about the selected window.
        """

        # Use the last app windows retrieved from get_desktop_app_info
        app_window_dict = ui_state.last_app_windows

        _verify_id(id, name, app_window_dict)

        # Find the window with the matching id
        window = app_window_dict.get(id)

        try:
            _ufoagent_restore_window_for_actions(window)

            if configs and configs.get("SHOW_VISUAL_OUTLINE_ON_SCREEN", True):
                window.draw_outline(colour="red", thickness=3)
        except Exception as e:
            raise ToolError(f"Failed to bring window on-screen: {str(e)}")

        # Initialize UI state for this window
        ui_state.initialize_for_window(window)
        ui_state.control_dict = _ufoagent_current_controls(ui_state)

        return {
            "root_name": ui_state.control_inspector.get_application_root_name(window),
            "window_info": _window2window_info(window).model_dump(),
        }

"#;
const EXECUTE_ACTION_FUNCTION: &str = r#"    def _execute_action(action: ActionCommandInfo) -> str:
        """
        Execute a single UI action.
        :param action: ActionCommandInfo object to execute.
        :return: Execution result as a dictionary.
        """
        if not ui_state.puppeteer or not ui_state.selected_app_window:
            raise ValueError(
                "UI state not initialized. Please select an application window first."
            )

        _ufoagent_restore_window_for_actions(ui_state.selected_app_window)
        ui_state.control_dict = _ufoagent_current_controls(ui_state)

        result = executor.execute(
            action,
            ui_state.puppeteer,
            ui_state.control_dict or {},
            ui_state.selected_app_window,
        )

        if not result:
            if not result:
                result = f"Executed action {action.action_string}, please check the application for whether it took effect."

        return result

"#;
const CLICK_INPUT_FUNCTION: &str = r#"    def click_input(
        id: Annotated[
            str,
            Field(
                description="The precise annotated ID of the selected control item to be clicked, adhering strictly to the provided options in the field of 'id' in the control information."
            ),
        ],
        name: Annotated[
            Optional[str],
            Field(
                description="The precise name of the selected control item to be clicked. If omitted, the current name for the selected id is used."
            ),
        ] = None,
        button: Annotated[
            str,
            Field(
                description="The mouse button to click. One of ''left'', ''right'', ''middle'' or ''x'"
            ),
        ] = "left",
        double: Annotated[
            bool, Field(description="Whether to perform a double click")
        ] = False,
    ) -> Annotated[str, Field(description="The result of the click action.")]:
        """
        Click on a UI control element using the mouse. All type of controls elements are supported.
        """

        ui_state.control_dict = _ufoagent_current_controls(ui_state)
        selected = ui_state.control_dict.get(id) if ui_state.control_dict else None
        true_name = selected.element_info.name if selected else None
        action_name = name or true_name or ""
        control_verified = _verify_id(id, action_name, ui_state.control_dict)

        action = ActionCommandInfo(
            function="click_input",
            arguments={"button": button, "double": double},
            target=TargetInfo(id=id, name=action_name, kind="control"),
        )

        result = _execute_action(action)

        if control_verified or name is None:
            return result
        else:
            true_name = ui_state.control_dict.get(id).element_info.name
            return f"Warning: The name of your chosen control id {id} is {true_name}, but the name argument is {name}. The action is performed on control {id}:{true_name}."

"#;
const KEYBOARD_INPUT_FUNCTION: &str = r#"    def keyboard_input(
        id: Annotated[
            str,
            Field(
                description="The annotated ID of the selected control item to send keyboard input to. If omitted, keys are sent to the selected application window."
            ),
        ] = "",
        keys: Annotated[
            Optional[str],
            Field(
                description="Key sequence to send. It can be any key on the keyboard, with special keys represented by their virtual key codes, for example, '{VK_CONTROL}c' for Ctrl+C."
            ),
        ] = None,
        name: Annotated[
            Optional[str],
            Field(
                description="The name of the selected control item. If omitted, the current name for the selected id is used."
            ),
        ] = None,
        text: Annotated[
            Optional[str],
            Field(
                description="Plain text to send. Accepted as an alias for keys for model-generated calls."
            ),
        ] = None,
        control_focus: Annotated[
            bool,
            Field(
                description="Whether to focus the selected control id before sending keys. If False or no id is provided, the hotkeys operate on the application window."
            ),
        ] = True,
    ) -> Annotated[
        str, Field(description="The key of the control item to send keyboard input to.")
    ]:
        """
        Simulate keyboard input to a control or the focused application, such as sending key presses or shortcuts.
        For example,
        - keyboard_input(keys="{VK_CONTROL}c") --> Copy the selected text
        - keyboard_input(keys="{TAB 2}") --> Press the Tab key twice.
        """
        actual_keys = keys if keys is not None else (text or "")
        if not actual_keys:
            raise ToolError("keyboard_input requires keys or text")

        ui_state.control_dict = _ufoagent_current_controls(ui_state)
        selected = ui_state.control_dict.get(id) if id and ui_state.control_dict else None
        target = None
        if selected:
            action_name = name or selected.element_info.name or ""
            target = TargetInfo(id=id, name=action_name, kind="control")
        else:
            control_focus = False

        action = ActionCommandInfo(
            function="keyboard_input",
            arguments={"keys": actual_keys, "control_focus": control_focus},
            target=target,
        )

        return _execute_action(action)

"#;
const GET_APP_WINDOW_CONTROLS_INFO_FUNCTION: &str = r#"    def get_app_window_controls_info(field_list: List[str]) -> List:
        """
        Get information about controls in the currently selected application window.
        :param field_list: List of fields to retrieve from the control info.
        :return: Dictionary containing the requested control information.
        """
        if not ui_state.selected_app_window:
            raise ToolError("No window is selected， please select a window first.")

        control_dict = _ufoagent_current_controls(ui_state)

        result = ui_state.control_inspector.get_control_info_list_of_dict(
            control_dict, field_list=field_list
        )

        ui_state.control_dict = control_dict

        return result

"#;
const GET_APP_WINDOW_CONTROLS_TARGET_INFO_FUNCTION: &str = r#"    def get_app_window_controls_target_info(field_list: List[str]) -> List:
        """
        Get information about controls in the currently selected application window.
        :param field_list: List of fields to retrieve from the control info.
        :return: Dictionary containing the requested control information.
        """
        if not ui_state.selected_app_window:
            raise ToolError("No window is selected， please select a window first.")

        control_dict = _ufoagent_current_controls(ui_state)

        ui_state.control_dict = control_dict

        target_info_list = []
        for id, control in control_dict.items():
            control_info = ui_state.control_inspector.get_control_info(
                control, field_list
            )
            target_info_list.append(
                TargetInfo(
                    kind=TargetKind.CONTROL,
                    id=str(id),
                    name=control_info.get("control_text"),
                    type=control_info.get("control_type"),
                    rect=control_info.get("control_rect"),
                    source="uia",
                )
            )

        return target_info_list

"#;
const CAPTURE_WINDOW_SCREENSHOT_FUNCTION: &str = r#"    def capture_window_screenshot() -> str:
        """
        Capture a screenshot of the currently selected application window.
        :return: Base64 encoded image data of the screenshot.
        """
        if not ui_state.selected_app_window:
            return "Error: No window selected"

        _ufoagent_restore_window_for_actions(ui_state.selected_app_window)

        try:
            screenshot = None

            # Attempt 1: capture the selected app window
            if ui_state.selected_app_window:
                try:
                    screenshot = ui_state.photographer.capture_app_window_screenshot(
                        ui_state.selected_app_window
                    )
                except Exception as win_err:
                    logger.warning(f"App window screenshot failed: {win_err}")

            # Validate screenshot
            if screenshot is not None:
                try:
                    w, h = screenshot.size
                    if w <= 1 or h <= 1:
                        logger.warning("App window screenshot too small, treating as invalid")
                        screenshot = None
                except Exception:
                    screenshot = None

            # Attempt 2: fall back to desktop screenshot
            if screenshot is None:
                logger.info("Falling back to desktop screenshot")
                screenshot = ui_state.photographer.capture_desktop_screen_screenshot(
                    all_screens=False
                )

            # Encode as base64
            screenshot_data = ui_state.photographer.encode_image(screenshot)

            return screenshot_data

        except Exception as e:
            return f"Error capturing screenshot: {str(e)}"

"#;

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
    // Every UFO2 agent role uses the one vended credential. Roles we leave unconfigured, notably
    // EVALUATION_AGENT, fall back to UFO2's Azure-AD default and crash after the task completes.
    ["HOST_AGENT", "APP_AGENT", "EVALUATION_AGENT", "BACKUP_AGENT"]
        .iter()
        .fold(
            "# Managed by ufoagent - do not edit by hand.\n# Short-lived credential, refreshed by the agent.\n".to_string(),
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

fn cli_mcp_server_path(ufo_home: &Path) -> PathBuf {
    ufo_home
        .join("ufo")
        .join("client")
        .join("mcp")
        .join("local_servers")
        .join("cli_mcp_server.py")
}

fn ui_mcp_server_path(ufo_home: &Path) -> PathBuf {
    ufo_home
        .join("ufo")
        .join("client")
        .join("mcp")
        .join("local_servers")
        .join("ui_mcp_server.py")
}

fn replace_python_function(
    original: &str,
    indent: &str,
    function_name: &str,
    replacement: &str,
) -> Option<String> {
    let needle = format!("{indent}def {function_name}(");
    let start = original.find(&needle)?;
    let rest = &original[start..];
    let mut end = rest.len();
    let mut offset = 0;
    let body_indent = format!("{indent}    ");
    for line in rest.split_inclusive('\n') {
        if offset > 0 {
            let trimmed = line.trim();
            if !trimmed.is_empty() && !line.starts_with(&body_indent) {
                end = offset;
                break;
            }
        }
        offset += line.len();
    }
    let mut patched = String::with_capacity(original.len() - end + replacement.len());
    patched.push_str(&original[..start]);
    patched.push_str(replacement);
    patched.push_str(&rest[end..]);
    Some(patched)
}

fn replace_top_level_python_function(
    original: &str,
    function_name: &str,
    replacement: &str,
) -> Option<String> {
    replace_python_function(original, "", function_name, replacement)
}

fn patch_cli_allowlist(path: &Path) -> Result<bool> {
    let original = std::fs::read_to_string(path)
        .with_context(|| format!("reading UFO CLI MCP server {}", path.display()))?;
    if original.contains(CLI_ALLOW_ALL_MARKER) {
        return Ok(false);
    }
    let Some(patched) = replace_top_level_python_function(
        &original,
        "_is_cli_command_allowed",
        CLI_ALLOW_ALL_FUNCTION,
    ) else {
        anyhow::bail!(
            "could not find _is_cli_command_allowed in UFO file {}",
            path.display()
        );
    };
    if patched == original {
        return Ok(false);
    }
    std::fs::write(path, patched)
        .with_context(|| format!("writing managed UFO CLI allowlist patch {}", path.display()))?;
    Ok(true)
}

fn patch_ui_action_primitives(path: &Path) -> Result<bool> {
    let original = std::fs::read_to_string(path)
        .with_context(|| format!("reading UFO UI MCP server {}", path.display()))?;
    if original.contains(GUI_ACTIONS_MARKER) {
        return Ok(false);
    }

    let mut patched = original.clone();
    if !patched.contains("import time\n") {
        let next = patched.replacen("import os\n", "import os\nimport time\n", 1);
        if next == patched {
            anyhow::bail!("could not add time import in UFO file {}", path.display());
        }
        patched = next;
    }
    let logger_line = "logger = logging.getLogger(__name__)\n";
    let Some(logger_end) = patched.find(logger_line).map(|idx| idx + logger_line.len()) else {
        anyhow::bail!(
            "could not find logger initialization in UFO file {}",
            path.display()
        );
    };
    patched.insert_str(logger_end, GUI_ACTIONS_HELPERS);
    for (function_name, replacement) in [
        (
            "select_application_window",
            SELECT_APPLICATION_WINDOW_FUNCTION,
        ),
        ("_execute_action", EXECUTE_ACTION_FUNCTION),
        ("click_input", CLICK_INPUT_FUNCTION),
        ("keyboard_input", KEYBOARD_INPUT_FUNCTION),
        (
            "get_app_window_controls_info",
            GET_APP_WINDOW_CONTROLS_INFO_FUNCTION,
        ),
        (
            "get_app_window_controls_target_info",
            GET_APP_WINDOW_CONTROLS_TARGET_INFO_FUNCTION,
        ),
        (
            "capture_window_screenshot",
            CAPTURE_WINDOW_SCREENSHOT_FUNCTION,
        ),
    ] {
        let Some(next) = replace_python_function(&patched, "    ", function_name, replacement)
        else {
            anyhow::bail!(
                "could not find {function_name} in UFO file {}",
                path.display()
            );
        };
        patched = next;
    }

    if patched == original {
        return Ok(false);
    }
    std::fs::write(path, patched)
        .with_context(|| format!("writing managed UFO UI action patch {}", path.display()))?;
    Ok(true)
}

/// Apply only the UFOAgent-owned runtime settings.
///
/// UFO's native mcp.yaml, prompt configuration, and MCP loading are intentionally left intact. We
/// use UFO's own SAFE_GUARD flag to avoid interactive confirmations, patch the hardcoded CLI MCP
/// allowlist so unattended installs can run the commands UFO chooses, and patch only UI action
/// primitives that otherwise report success while missing the real on-screen target.
pub fn apply_unattended_mode(ufo_home: &Path) -> Result<PathBuf> {
    let cli_mcp = cli_mcp_server_path(ufo_home);
    if cli_mcp.exists() {
        patch_cli_allowlist(&cli_mcp)?;
    }
    let ui_mcp = ui_mcp_server_path(ufo_home);
    if ui_mcp.exists() {
        patch_ui_action_primitives(&ui_mcp)?;
    }

    let path = system_yaml_path(ufo_home);
    let original = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e.into()),
    };

    let mut saw_safe_guard = false;
    let mut changed = false;
    let mut lines = Vec::new();

    for line in original.lines() {
        if line.trim_start().starts_with("SAFE_GUARD:") {
            saw_safe_guard = true;
            if line != UNATTENDED_SAFE_GUARD_LINE {
                changed = true;
            }
            lines.push(UNATTENDED_SAFE_GUARD_LINE.to_string());
        } else {
            lines.push(line.to_string());
        }
    }

    if !saw_safe_guard {
        if !lines.is_empty() && lines.last().is_some_and(|l| !l.trim().is_empty()) {
            lines.push(String::new());
        }
        lines.push("# UFOAgent unattended mode".to_string());
        lines.push(UNATTENDED_SAFE_GUARD_LINE.to_string());
        changed = true;
    }

    let mut rendered = lines.join("\n");
    rendered.push('\n');
    if changed || rendered != original {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
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
        for role in [
            "HOST_AGENT:",
            "APP_AGENT:",
            "EVALUATION_AGENT:",
            "BACKUP_AGENT:",
        ] {
            assert!(y.contains(role), "missing {role}");
        }
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
    fn unattended_mode_only_disables_safe_guard() {
        let home = temp_home("safe-guard-existing");
        let system = system_yaml_path(&home);
        std::fs::create_dir_all(system.parent().unwrap()).unwrap();
        std::fs::write(
            &system,
            "# UFO System Configuration\nUSE_MCP: True\nSAFE_GUARD: True  # upstream default\nMCP_FALLBACK_TO_UI: True\n",
        )
        .unwrap();

        let path = apply_unattended_mode(&home).unwrap();
        let y = std::fs::read_to_string(&path).unwrap();
        assert!(y.contains(UNATTENDED_SAFE_GUARD_LINE));
        assert!(y.contains("USE_MCP: True"));
        assert!(y.contains("MCP_FALLBACK_TO_UI: True"));
        assert!(!y.contains("SAFE_GUARD: True"));

        apply_unattended_mode(&home).unwrap();
        let y = std::fs::read_to_string(&path).unwrap();
        assert_eq!(y.matches("SAFE_GUARD:").count(), 1);

        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn unattended_mode_creates_system_yaml_when_missing() {
        let home = temp_home("safe-guard-missing");

        let path = apply_unattended_mode(&home).unwrap();
        let y = std::fs::read_to_string(&path).unwrap();
        assert!(y.contains("# UFOAgent unattended mode"));
        assert!(y.contains(UNATTENDED_SAFE_GUARD_LINE));

        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn cli_allowlist_patch_allows_unattended_commands() {
        let home = temp_home("cli-allowlist");
        let cli = cli_mcp_server_path(&home);
        std::fs::create_dir_all(cli.parent().unwrap()).unwrap();
        std::fs::write(
            &cli,
            r#"import shlex

ALLOWED_CLI_COMMANDS = frozenset({"notepad"})

def _is_cli_command_allowed(command_str: str) -> bool:
    if not command_str or not command_str.strip():
        return False
    tokens = shlex.split(command_str)
    return tokens[0] in ALLOWED_CLI_COMMANDS

@MCPRegistry.register_factory_decorator("CommandLineExecutor")
def create_cli_mcp_server(*args, **kwargs):
    pass
"#,
        )
        .unwrap();

        assert!(patch_cli_allowlist(&cli).unwrap());
        let patched = std::fs::read_to_string(&cli).unwrap();
        assert!(patched.contains(CLI_ALLOW_ALL_MARKER));
        assert!(patched.contains("return bool(command_str and command_str.strip())"));
        assert!(!patched.contains("return tokens[0] in ALLOWED_CLI_COMMANDS"));
        assert!(patched.contains("@MCPRegistry.register_factory_decorator"));

        assert!(!patch_cli_allowlist(&cli).unwrap());
        let patched_again = std::fs::read_to_string(&cli).unwrap();
        assert_eq!(patched, patched_again);

        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn ui_action_patch_makes_keyboard_and_windows_tolerant() {
        let home = temp_home("ui-actions");
        let ui = ui_mcp_server_path(&home);
        std::fs::create_dir_all(ui.parent().unwrap()).unwrap();
        std::fs::write(
            &ui,
            r#"import os
from typing import Annotated, Any, Dict, List, Optional

logger = logging.getLogger(__name__)


@MCPRegistry.register_factory_decorator("HostUIExecutor")
def create_host_action_mcp_server(*args, **kwargs) -> FastMCP:
    def select_application_window(
        id: Annotated[str, "id"],
        name: Annotated[str, "name"],
    ) -> Dict[str, Any]:
        window.set_focus()
        return {"window_info": _window2window_info(window).model_dump()}

    return action_mcp

@MCPRegistry.register_factory_decorator("AppUIExecutor")
def create_app_action_mcp_server(*args, **kwargs) -> FastMCP:
    def _execute_action(action: ActionCommandInfo) -> str:
        result = executor.execute(action, ui_state.puppeteer, ui_state.control_dict or {}, ui_state.selected_app_window)
        return result

    def click_input(
        id: Annotated[str, Field(description="id")],
        name: Annotated[str, Field(description="name")],
        button: Annotated[str, Field(description="button")] = "left",
        double: Annotated[bool, Field(description="double")] = False,
    ) -> Annotated[str, Field(description="result")]:
        control_verified = _verify_id(id, name, ui_state.control_dict)
        return _execute_action(action)

    def keyboard_input(
        id: Annotated[str, Field(description="id")],
        name: Annotated[str, Field(description="name")],
        keys: Annotated[str, Field(description="keys")],
        control_focus: Annotated[bool, Field(description="focus")] = True,
    ) -> Annotated[str, Field(description="result")]:
        return _execute_action(action)

    return action_mcp

@MCPRegistry.register_factory_decorator("UICollector")
def create_data_mcp_server(*args, **kwargs) -> FastMCP:
    def get_app_window_controls_info(field_list: List[str]) -> List:
        controls_list = ui_state.control_inspector.find_control_elements_in_descendants(ui_state.selected_app_window)
        control_dict = {str(i + 1): control for i, control in enumerate(controls_list)}
        return []

    def get_app_window_controls_target_info(field_list: List[str]) -> List:
        controls_list = ui_state.control_inspector.find_control_elements_in_descendants(ui_state.selected_app_window)
        control_dict = {str(i + 1): control for i, control in enumerate(controls_list)}
        return []

    def capture_window_screenshot() -> str:
        return ui_state.photographer.capture_app_window_screenshot(ui_state.selected_app_window)
"#,
        )
        .unwrap();

        assert!(patch_ui_action_primitives(&ui).unwrap());
        let patched = std::fs::read_to_string(&ui).unwrap();
        assert!(patched.contains(GUI_ACTIONS_MARKER));
        assert!(patched.contains("window.maximize()"));
        assert!(patched.contains("text: Annotated["));
        assert!(patched.contains("actual_keys = keys if keys is not None else (text or \"\")"));
        assert!(patched.contains("_ufoagent_current_controls(ui_state)"));
        assert!(patched.contains("name: Annotated[\n            Optional[str],"));
        assert!(patched.contains("def capture_window_screenshot() -> str:"));
        assert!(patched.contains("App window screenshot too small, treating as invalid"));
        assert!(patched.contains("@MCPRegistry.register_factory_decorator(\"AppUIExecutor\")"));
        assert!(patched.contains("def create_app_action_mcp_server"));
        assert!(patched.contains("    return action_mcp"));

        assert!(!patch_ui_action_primitives(&ui).unwrap());
        let patched_again = std::fs::read_to_string(&ui).unwrap();
        assert_eq!(patched, patched_again);

        let _ = std::fs::remove_dir_all(home);
    }
}
