"""PPO — clipped actor-critic RL fine-tuner (CleanRL-style, single file).

An alternative to GRPO that learns a value baseline and reuses each batch of
rollouts for several clipped-surrogate epochs. Returns-to-go are the value
targets; advantage = return-to-go - V(s), normalised per batch.
"""

from __future__ import annotations

import numpy as np

from gui_rl.algos.rollout import returns_to_go, sample_episode
from gui_rl.env import WindowsGuiEnv
from gui_rl.policies.base import Adam
from gui_rl.policies.linear import LinearGuiPolicy, stack_features
from gui_rl.tasks import Task


def ppo_finetune(
    policy: LinearGuiPolicy,
    tasks: list[Task],
    iterations: int = 60,
    rollouts_per_task: int = 8,
    epochs: int = 4,
    batch_size: int = 256,
    lr: float = 1e-2,
    clip: float = 0.2,
    gamma: float = 0.99,
    vf_coef: float = 0.5,
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
        feats, acts, old_logps, rtgs = [], [], [], []
        succ = []
        for env in envs:
            for _ in range(rollouts_per_task):
                ep = sample_episode(env, policy, rng)
                feats.extend(ep["features"])
                acts.append(ep["actions"])
                old_logps.append(ep["logps"])
                rtgs.append(returns_to_go(ep["rewards"], gamma))
                succ.append(ep["success"])
        actions = np.concatenate(acts)
        old_logp = np.concatenate(old_logps)
        returns = np.concatenate(rtgs)

        # Old value estimates for advantage (recomputed once, off the sampling policy).
        o0 = policy.forward(stack_features(feats))
        values = o0.value.copy()
        adv = returns - values
        adv = (adv - adv.mean()) / (adv.std() + 1e-8)

        n = len(actions)
        for _ in range(epochs):
            idx = rng.permutation(n)
            for s in range(0, n, batch_size):
                b = idx[s:s + batch_size]
                ob = policy.forward(stack_features([feats[j] for j in b]))
                grads = policy.ppo_grads(
                    ob, actions[b], old_logp[b], adv[b], returns[b],
                    clip=clip, vf_coef=vf_coef, ent_coef=ent_coef,
                )
                opt.step(grads)

        m = {"iter": it, "success": float(np.mean(succ))}
        history.append(m)
        if verbose:
            print(f"[ppo] iter {it:3d}  success {m['success']:.3f}")
    return history


def _mock(task: Task, repair_a11y: bool):
    from gui_rl.backends.mock import MockGuiBackend
    return MockGuiBackend(lambda seed: task.scenario(seed), repair_a11y=repair_a11y)
