"""GRPO — Group Relative Policy Optimization (the primary RL fine-tuner).

GRPO drops the value critic: for each task it samples a *group* of episodes,
then uses the **group-normalised return** as each episode's advantage. With a
trustworthy instrumentation reward oracle (success = +1), this is a clean,
low-variance way to push success rate up after behavior cloning — and it is what
current GUI/agent RL fine-tuning largely uses.

advantage_i = (R_i - mean(R_group)) / (std(R_group) + eps), broadcast to every
step of episode i. One on-policy gradient step per iteration.
"""

from __future__ import annotations

import numpy as np

from gui_rl.algos.rollout import sample_episode
from gui_rl.env import WindowsGuiEnv
from gui_rl.policies.base import Adam
from gui_rl.policies.linear import LinearGuiPolicy, stack_features
from gui_rl.tasks import Task


def grpo_finetune(
    policy: LinearGuiPolicy,
    tasks: list[Task],
    iterations: int = 60,
    group_size: int = 8,
    lr: float = 2e-2,
    ent_coef: float = 0.01,
    repair_a11y: bool = True,
    seed: int = 0,
    verbose: bool = False,
) -> list[dict]:
    opt = Adam(policy.p, lr=lr)
    rng = np.random.default_rng(seed)
    envs = [WindowsGuiEnv(t, backend=_mock(t, repair_a11y)) for t in tasks]
    history = []

    for it in range(iterations):
        feats_all, acts_all, adv_all = [], [], []
        succ = []
        for env in envs:
            group = [sample_episode(env, policy, rng) for _ in range(group_size)]
            rets = np.array([g["ret"] for g in group])
            adv = (rets - rets.mean()) / (rets.std() + 1e-8)
            for gi, g in enumerate(group):
                feats_all.extend(g["features"])
                acts_all.append(g["actions"])
                adv_all.append(np.full(len(g["actions"]), adv[gi]))
                succ.append(g["success"])
        actions = np.concatenate(acts_all)
        advs = np.concatenate(adv_all)
        o = policy.forward(stack_features(feats_all))
        grads = policy.pg_grads(o, actions, advs, ent_coef=ent_coef)
        opt.step(grads)

        m = {"iter": it, "success": float(np.mean(succ))}
        history.append(m)
        if verbose:
            print(f"[grpo] iter {it:3d}  success {m['success']:.3f}")
    return history


def _mock(task: Task, repair_a11y: bool):
    from gui_rl.backends.mock import MockGuiBackend
    return MockGuiBackend(lambda seed: task.scenario(seed), repair_a11y=repair_a11y)
