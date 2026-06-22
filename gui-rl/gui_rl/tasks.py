"""Task suite + reward shaping.

A :class:`Task` defines the natural-language instruction, the mock scenario
(screen graph) used for training/eval, success/progress predicates, and a
*scripted expert* used to bootstrap behavior-cloning data. Reward is generic and
potential-shaped: a per-step time penalty, milestone bonuses for making progress
toward the goal, a large terminal reward for finishing on success, and a penalty
for declaring ``finish`` prematurely.

Success / progress / expert read the mock backend's ground-truth state from
``obs.extra``. Real-Windows tasks would override these three to inspect the
accessibility tree / screenshot instead — the rest of the stack is unchanged.
"""

from __future__ import annotations

import abc
from dataclasses import dataclass
from typing import Optional

from gui_rl.actions import GuiAction
from gui_rl.backends.base import Observation
from gui_rl.backends.mock import ElementSpec, Scenario, Screen, Transition


@dataclass
class RewardConfig:
    step_penalty: float = 0.02
    success_reward: float = 1.0
    progress_reward: float = 0.25
    premature_finish_penalty: float = 0.3


def _find(obs: Observation, name: str) -> Optional[int]:
    for i, el in enumerate(obs.elements):
        if el.name == name:
            return i
    return None


def _act(obs: Observation, verb: str, name: str | None = None, param: str = "") -> GuiAction:
    eid = _find(obs, name) if name is not None else None
    return GuiAction(verb=verb, element_id=eid, param=param)


class Task(abc.ABC):
    name: str = "task"
    max_steps: int = 12
    reward = RewardConfig()

    @abc.abstractmethod
    def scenario(self, seed: Optional[int] = None) -> Scenario: ...

    @abc.abstractmethod
    def is_success(self, obs: Observation) -> bool: ...

    def progress(self, obs: Observation) -> int:
        """Monotone-ish milestone count for reward shaping (0..n)."""
        return 0

    @abc.abstractmethod
    def expert(self, obs: Observation) -> GuiAction:
        """A scripted oracle that solves the task — used to generate BC data."""

    # Reward/success read the privileged instrumentation signals (the oracle),
    # never the policy-visible observation.
    @staticmethod
    def _state(obs: Observation) -> dict:
        return obs.signals

    @staticmethod
    def _screen(obs: Observation) -> str:
        return obs.signals.get("screen", "")


class OpenCalculatorTask(Task):
    name = "open_calculator"
    max_steps = 6

    def scenario(self, seed=None) -> Scenario:
        return Scenario(
            instruction="Open the Calculator app.",
            start_screen="desktop",
            init_state={"opened": False},
            screens={
                "desktop": Screen("desktop", [
                    ElementSpec("Start", "Button"),
                    ElementSpec("Recycle Bin", "Button"),  # distractor
                ]),
                "start_menu": Screen("start_menu", [
                    ElementSpec("Calculator", "MenuItem"),
                    ElementSpec("Settings", "MenuItem"),   # distractor
                    ElementSpec("Notepad", "MenuItem"),    # distractor
                ]),
                # Launching changes the screen — success is observable, as on a
                # real desktop where the Calculator window appears.
                "calculator": Screen("calculator", [
                    ElementSpec("Display", "Text"),
                    ElementSpec("Equals", "Button"),
                ]),
            },
            transitions=[
                Transition("desktop", "click", "Start", to_screen="start_menu"),
                Transition("start_menu", "click", "Calculator",
                           to_screen="calculator", set_state={"opened": True}),
            ],
        )

    def is_success(self, obs) -> bool:
        return bool(self._state(obs).get("opened"))

    def progress(self, obs) -> int:
        return {"desktop": 0, "start_menu": 1, "calculator": 2}.get(self._screen(obs), 0)

    def expert(self, obs) -> GuiAction:
        screen = self._screen(obs)
        if screen == "calculator":
            return _act(obs, "finish")
        if screen == "desktop":
            return _act(obs, "click", "Start")
        return _act(obs, "click", "Calculator")


class EnableDarkModeTask(Task):
    name = "enable_dark_mode"
    max_steps = 10

    def scenario(self, seed=None) -> Scenario:
        return Scenario(
            instruction="Open Settings and turn on dark mode.",
            start_screen="desktop",
            init_state={"dark_mode": False},
            screens={
                "desktop": Screen("desktop", [
                    ElementSpec("Start", "Button"),
                    ElementSpec("Recycle Bin", "Button"),
                ]),
                "start_menu": Screen("start_menu", [
                    ElementSpec("Settings", "MenuItem"),
                    ElementSpec("Calculator", "MenuItem"),
                    ElementSpec("Notepad", "MenuItem"),
                ]),
                "settings": Screen("settings", [
                    ElementSpec("Personalization", "Button"),
                    ElementSpec("System", "Button"),
                    ElementSpec("Back", "Button"),
                ]),
                "personalization": Screen("personalization", [
                    # Custom-drawn toggle: invisible to UIA, only an instrumented
                    # launch recovers it. With repair off the agent is stuck here.
                    ElementSpec("Dark Mode", "CheckBox", checked_key="dark_mode",
                                a11y_visible=False),
                    ElementSpec("Light Mode", "RadioButton"),
                    ElementSpec("Back", "Button"),
                ]),
            },
            transitions=[
                Transition("desktop", "click", "Start", to_screen="start_menu"),
                Transition("start_menu", "click", "Settings", to_screen="settings"),
                Transition("settings", "click", "Personalization", to_screen="personalization"),
                Transition("settings", "click", "Back", to_screen="start_menu"),
                Transition("personalization", "click", "Dark Mode", set_state={"dark_mode": True}),
                Transition("personalization", "click", "Back", to_screen="settings"),
            ],
        )

    def is_success(self, obs) -> bool:
        return bool(self._state(obs).get("dark_mode"))

    def progress(self, obs) -> int:
        order = {"desktop": 0, "start_menu": 1, "settings": 2, "personalization": 3}
        p = order.get(self._screen(obs), 0)
        return p + (1 if self.is_success(obs) else 0)

    def expert(self, obs) -> GuiAction:
        if self.is_success(obs):
            return _act(obs, "finish")
        screen = self._screen(obs)
        nxt = {
            "desktop": ("Start",),
            "start_menu": ("Settings",),
            "settings": ("Personalization",),
            "personalization": ("Dark Mode",),
        }[screen]
        return _act(obs, "click", nxt[0])


class TypeAndSaveTask(Task):
    name = "type_and_save"
    max_steps = 10

    def scenario(self, seed=None) -> Scenario:
        return Scenario(
            instruction="Open Notepad, type 'hello world', and save the file.",
            start_screen="desktop",
            init_state={"typed": False, "saved": False},
            screens={
                "desktop": Screen("desktop", [
                    ElementSpec("Start", "Button"),
                    ElementSpec("Recycle Bin", "Button"),
                ]),
                "start_menu": Screen("start_menu", [
                    ElementSpec("Notepad", "MenuItem"),
                    ElementSpec("Settings", "MenuItem"),
                    ElementSpec("Calculator", "MenuItem"),
                ]),
                # Editing then saving each advance the screen, so the agent can
                # observe its own progress (empty doc -> typed -> saved).
                "notepad": Screen("notepad", [
                    ElementSpec("Document", "Edit", checked_key="typed"),
                    ElementSpec("Save", "Button"),
                    ElementSpec("Close", "Button"),
                ]),
                "notepad_typed": Screen("notepad_typed", [
                    ElementSpec("Document", "Edit", checked_key="typed"),
                    ElementSpec("Save", "Button", checked_key="saved"),
                    ElementSpec("Close", "Button"),
                ]),
                "notepad_saved": Screen("notepad_saved", [
                    ElementSpec("Document", "Edit", checked_key="typed"),
                    ElementSpec("Save", "Button", checked_key="saved"),
                    ElementSpec("Saved", "Text"),
                ]),
            },
            transitions=[
                Transition("desktop", "click", "Start", to_screen="start_menu"),
                Transition("start_menu", "click", "Notepad", to_screen="notepad"),
                Transition("notepad", "type", "Document", param="hello world",
                           to_screen="notepad_typed", set_state={"typed": True}),
                Transition("notepad_typed", "click", "Save",
                           to_screen="notepad_saved", set_state={"saved": True}),
                Transition("notepad_typed", "key", param="ctrl+s",
                           to_screen="notepad_saved", set_state={"saved": True}),
            ],
        )

    def is_success(self, obs) -> bool:
        s = self._state(obs)
        return bool(s.get("typed") and s.get("saved"))

    def progress(self, obs) -> int:
        return {"desktop": 0, "start_menu": 1, "notepad": 2,
                "notepad_typed": 3, "notepad_saved": 4}.get(self._screen(obs), 0)

    def expert(self, obs) -> GuiAction:
        screen = self._screen(obs)
        if screen == "desktop":
            return _act(obs, "click", "Start")
        if screen == "start_menu":
            return _act(obs, "click", "Notepad")
        if screen == "notepad":
            return _act(obs, "type", "Document", param="hello world")
        if screen == "notepad_typed":
            return _act(obs, "click", "Save")
        return _act(obs, "finish")  # notepad_saved


# Registry ---------------------------------------------------------------
TASK_CLASSES = {
    t.name: t
    for t in (OpenCalculatorTask, EnableDarkModeTask, TypeAndSaveTask)
}


def make_task(name: str) -> Task:
    if name not in TASK_CLASSES:
        raise KeyError(f"unknown task {name!r}; choices: {sorted(TASK_CLASSES)}")
    return TASK_CLASSES[name]()


def all_tasks() -> list[Task]:
    return [cls() for cls in TASK_CLASSES.values()]
