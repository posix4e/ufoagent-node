import numpy as np

from gui_rl.algos.bc import behavior_clone
from gui_rl.algos.grpo import grpo_finetune
from gui_rl.algos.ppo import ppo_finetune
from gui_rl.data import TrajectoryDataset, collect
from gui_rl.env import WindowsGuiEnv
from gui_rl.evaluate import evaluate
from gui_rl.policies.linear import LinearGuiPolicy
from gui_rl.tasks import OpenCalculatorTask, all_tasks


def test_bc_reduces_loss_and_solves_task():
    t = OpenCalculatorTask()
    trajs = collect(WindowsGuiEnv(t), 60)
    policy = LinearGuiPolicy(seed=0)
    hist = behavior_clone(policy, TrajectoryDataset(trajs), epochs=25, seed=0)
    assert hist[-1]["loss"] < hist[0]["loss"]
    assert evaluate(policy, tasks=[t], episodes=20)["mean"] == 1.0


def test_grpo_improves_undertrained_policy():
    tasks = all_tasks()
    trajs = []
    for t in tasks:
        trajs += collect(WindowsGuiEnv(t), 40)
    policy = LinearGuiPolicy(seed=0)
    behavior_clone(policy, TrajectoryDataset(trajs), epochs=2, lr=5e-2, seed=0)
    before = evaluate(policy, episodes=30)["mean"]
    grpo_finetune(policy, tasks, iterations=120, group_size=10, lr=3e-2, seed=0)
    after = evaluate(policy, episodes=30)["mean"]
    assert after > before


def test_ppo_runs_and_returns_history():
    t = OpenCalculatorTask()
    policy = LinearGuiPolicy(seed=0)
    hist = ppo_finetune(policy, [t], iterations=5, rollouts_per_task=6, seed=0)
    assert len(hist) == 5 and "success" in hist[0]


def test_policy_act_shapes():
    env = WindowsGuiEnv(OpenCalculatorTask())
    feats, _ = env.reset(seed=0)
    policy = LinearGuiPolicy(seed=0)
    action, logp, value = policy.act(feats)
    assert action.shape == (3,)
    assert np.isfinite(logp) and np.isfinite(value)
