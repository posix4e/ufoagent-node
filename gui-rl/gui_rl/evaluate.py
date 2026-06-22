"""Evaluation harness: success rate over the task suite.

Supports the headline ablation for the instrumentation idea — evaluate the same
policy with the a11y tree *repaired* by instrumentation vs. the lossy raw UIA
view — so the value of dynamic instrumentation is a number, not a claim.
"""

from __future__ import annotations

import numpy as np

from gui_rl.algos.rollout import sample_episode
from gui_rl.backends.mock import MockGuiBackend
from gui_rl.env import WindowsGuiEnv
from gui_rl.tasks import Task, all_tasks


def evaluate(
    policy,
    tasks: list[Task] | None = None,
    episodes: int = 20,
    deterministic: bool = True,
    repair_a11y: bool = True,
    seed: int = 1234,
) -> dict:
    tasks = tasks or all_tasks()
    rng = np.random.default_rng(seed)
    per_task = {}
    for t in tasks:
        env = WindowsGuiEnv(
            t, backend=MockGuiBackend(lambda s: t.scenario(s), repair_a11y=repair_a11y)
        )
        wins = sum(
            sample_episode(env, policy, rng, deterministic=deterministic)["success"]
            for _ in range(episodes)
        )
        per_task[t.name] = wins / episodes
    per_task["mean"] = float(np.mean([v for k, v in per_task.items() if k != "mean"]))
    return per_task
