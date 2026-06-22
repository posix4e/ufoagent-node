"""``WindowsGuiEnv`` — a Gymnasium environment for grounded GUI control.

* **Observation** (``spaces.Dict``): ``screenshot`` (pixels), ``elements`` +
  ``element_mask`` (featurised accessibility tree), ``instruction`` (hashed
  task text). The raw a11y tree and instruction string ride along in ``info``
  for VLM policies and logging.
* **Action** (``spaces.MultiDiscrete([verbs, MAX_ELEMENTS, params])``): decoded
  against the live a11y tree into a :class:`~gui_rl.actions.GuiAction`.
* **Reward**: per-step time penalty + milestone shaping + terminal success
  bonus, configured per task (see :class:`~gui_rl.tasks.RewardConfig`).

The env is backend-agnostic. Construct it with the mock backend for
training/CI, or the Windows backend for deployment — the policy never changes.
"""

from __future__ import annotations

from typing import Optional

import gymnasium as gym
import numpy as np
from gymnasium import spaces

from gui_rl import actions as A
from gui_rl.backends.base import GuiBackend
from gui_rl.backends.mock import MockGuiBackend
from gui_rl.features import ELEM_FEAT, HASH_DIM, MAX_ELEMENTS, featurize
from gui_rl.tasks import Task


class WindowsGuiEnv(gym.Env):
    metadata = {"render_modes": []}

    def __init__(self, task: Task, backend: Optional[GuiBackend] = None):
        super().__init__()
        self.task = task
        # Default to the mock backend driven by this task's scenario.
        self.backend = backend or MockGuiBackend(lambda seed: task.scenario(seed))
        h, w = self.backend.screen_hw

        self.observation_space = spaces.Dict({
            "screenshot": spaces.Box(0, 255, (h, w, 3), dtype=np.uint8),
            "elements": spaces.Box(-1.0, 1.0, (MAX_ELEMENTS, ELEM_FEAT), dtype=np.float32),
            "element_mask": spaces.Box(0, 1, (MAX_ELEMENTS,), dtype=np.int8),
            "instruction": spaces.Box(-1.0, 1.0, (HASH_DIM,), dtype=np.float32),
        })
        self.action_space = spaces.MultiDiscrete([A.NUM_VERBS, MAX_ELEMENTS, A.NUM_PARAMS])

        self._obs = None
        self._steps = 0
        self._best_progress = 0

    # -- Gymnasium API ----------------------------------------------------
    def reset(self, *, seed: Optional[int] = None, options=None):
        super().reset(seed=seed)
        self._obs = self.backend.reset(self.task.scenario(seed).instruction, seed)
        self._steps = 0
        self._best_progress = self.task.progress(self._obs)
        return featurize(self._obs), self._info(A.GuiAction("wait"), {})

    def step(self, action):
        gui_action = A.decode(action, self._obs.elements)
        exec_info = self.backend.execute(gui_action)
        self._obs = self.backend.observe()
        self._steps += 1

        rc = self.task.reward
        reward = -rc.step_penalty

        # Potential-based shaping: reward newly reached milestones once.
        prog = self.task.progress(self._obs)
        if prog > self._best_progress:
            reward += rc.progress_reward * (prog - self._best_progress)
            self._best_progress = prog

        success = self.task.is_success(self._obs)
        terminated = False
        if gui_action.verb == "finish":
            terminated = True
            reward += rc.success_reward if success else -rc.premature_finish_penalty

        truncated = self._steps >= self.task.max_steps
        info = self._info(gui_action, exec_info)
        info["is_success"] = success and terminated
        return featurize(self._obs), float(reward), terminated, truncated, info

    # -- helpers ----------------------------------------------------------
    def action_mask(self) -> np.ndarray:
        """Boolean mask over element slots that point at a real control."""
        mask = np.zeros(MAX_ELEMENTS, dtype=bool)
        mask[: min(len(self._obs.elements), MAX_ELEMENTS)] = True
        return mask

    def expert_action(self) -> np.ndarray:
        """The scripted expert's action as MultiDiscrete indices (for BC data)."""
        return np.array(self.task.expert(self._obs).to_indices(), dtype=np.int64)

    def raw_observation(self):
        """The current backend :class:`Observation` (full fidelity, incl.
        privileged channels). Used by the trajectory collector."""
        return self._obs

    def _info(self, gui_action, exec_info: dict) -> dict:
        return {
            "instruction": self._obs.instruction,
            "a11y": [
                {
                    "name": e.name,
                    "control_type": e.control_type,
                    "bbox": e.bbox,
                    "enabled": e.enabled,
                    "checked": e.checked,
                }
                for e in self._obs.elements
            ],
            "action": gui_action.to_json(),
            "backend": exec_info,
            "screen": self._obs.signals.get("screen"),
            "signals": dict(self._obs.signals),     # privileged (for logging/oracle, not policy)
            "events": list(self._obs.events),        # privileged instrumentation event log
            "num_elements": len(self._obs.elements),
        }
