"""Backend abstraction: the bridge between the RL environment and a desktop.

A backend exposes the two things a GUI policy observes — a *screenshot* and an
*accessibility tree* — and executes the grounded :class:`~gui_rl.actions.GuiAction`
the policy chose. Everything above this interface (env, tasks, rewards, training)
is backend-agnostic, so the exact same policy can be trained against the mock
desktop and then deployed against a real Windows machine.
"""

from __future__ import annotations

import abc
from dataclasses import dataclass, field
from typing import Optional

import numpy as np

# Accessibility control types we featurise. Kept compact; UI Automation defines
# ~40 ControlTypeIds but a handful covers the overwhelming majority of actions.
CONTROL_TYPES = [
    "Unknown",
    "Button",
    "MenuItem",
    "Edit",
    "Text",
    "CheckBox",
    "RadioButton",
    "ListItem",
    "TabItem",
    "Window",
    "Hyperlink",
    "ComboBox",
]
CONTROL_TYPE_INDEX = {c: i for i, c in enumerate(CONTROL_TYPES)}


@dataclass
class Element:
    """One node of the accessibility tree (a UI Automation element)."""

    name: str
    control_type: str = "Unknown"
    bbox: tuple[float, float, float, float] = (0.0, 0.0, 0.0, 0.0)  # x, y, w, h normalised 0..1
    enabled: bool = True
    focused: bool = False
    checked: bool = False
    selected: bool = False
    automation_id: str = ""

    def control_type_index(self) -> int:
        return CONTROL_TYPE_INDEX.get(self.control_type, 0)


@dataclass
class Observation:
    """One step of perception.

    ``screenshot`` + ``elements`` (+ ``instruction``) are the *policy-visible*
    channels. ``signals`` and ``events`` are *privileged*: they come from
    dynamic instrumentation of the launched binary (see
    :mod:`gui_rl.backends.instrument`) and are used only by the reward/success
    oracle and for a11y repair — never fed to the policy. This keeps training
    asymmetric: rich supervision at train time, pixels+a11y at deployment.
    """

    screenshot: np.ndarray            # uint8 HxWx3  (policy-visible)
    elements: list[Element]           # accessibility tree, possibly repaired (policy-visible)
    instruction: str                  # the task in natural language (policy-visible)
    signals: dict = field(default_factory=dict)   # privileged ground-truth state from the hook
    events: list = field(default_factory=list)     # privileged structured events since last action
    extra: dict = field(default_factory=dict)


class GuiBackend(abc.ABC):
    """Drives a desktop. Implementations: mock (simulated) and windows_uia (real)."""

    #: Screenshot resolution the backend renders/returns (H, W).
    screen_hw: tuple[int, int] = (84, 84)

    @abc.abstractmethod
    def reset(self, instruction: str, seed: Optional[int] = None) -> Observation:
        """Reset the desktop to a task's initial state and return the first obs."""

    @abc.abstractmethod
    def observe(self) -> Observation:
        """Return the current observation without changing state."""

    @abc.abstractmethod
    def execute(self, action) -> dict:
        """Apply a :class:`~gui_rl.actions.GuiAction`. Returns an info dict.

        The info dict may include backend signals such as ``{"ok": bool,
        "no_op": bool}`` that reward functions and loggers can use.
        """

    def close(self) -> None:  # pragma: no cover - optional cleanup hook
        pass
