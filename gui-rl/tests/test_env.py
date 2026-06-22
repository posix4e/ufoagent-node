import numpy as np

from gui_rl import actions as A
from gui_rl.env import WindowsGuiEnv
from gui_rl.features import MAX_ELEMENTS
from gui_rl.tasks import EnableDarkModeTask, OpenCalculatorTask


def test_obs_and_action_spaces():
    env = WindowsGuiEnv(OpenCalculatorTask())
    obs, info = env.reset(seed=0)
    assert set(obs) == {"screenshot", "elements", "element_mask", "instruction"}
    assert obs["elements"].shape[0] == MAX_ELEMENTS
    assert env.action_space.contains(env.action_space.sample())
    assert "a11y" in info and "instruction" in info


def test_expert_solves_open_calculator():
    env = WindowsGuiEnv(OpenCalculatorTask())
    env.reset(seed=0)
    done = False
    success = False
    for _ in range(env.task.max_steps):
        _, _, term, trunc, info = env.step(env.expert_action())
        success = success or info["is_success"]
        if term or trunc:
            break
    assert success


def test_reward_shaping_and_premature_finish():
    env = WindowsGuiEnv(OpenCalculatorTask())
    env.reset(seed=0)
    # Finishing immediately fails -> negative terminal reward.
    _, r, term, _, info = env.step([A.VERB_INDEX["finish"], 0, 0])
    assert term and not info["is_success"] and r < 0


def test_action_mask_matches_tree():
    env = WindowsGuiEnv(EnableDarkModeTask())
    env.reset(seed=0)
    n = len(env.raw_observation().elements)
    mask = env.action_mask()
    assert mask[:n].all()
    assert not mask[n:].any()
