"""Shared on-policy sampling for the RL fine-tuners and evaluation."""

from __future__ import annotations

import numpy as np

from gui_rl.env import WindowsGuiEnv
from gui_rl.features import featurize


def sample_episode(env: WindowsGuiEnv, policy, rng, deterministic: bool = False):
    """Run one episode, capturing per-step features for the policy update.

    Returns a dict of parallel arrays/lists:
    ``features`` (list of feature dicts), ``actions``, ``rewards``,
    ``logps`` (old log-probs), ``values`` (old value estimates),
    plus scalar ``ret`` and bool ``success``.
    """
    env.reset()
    feats, acts, rews, logps, vals = [], [], [], [], []
    ret, success = 0.0, False
    for _ in range(env.task.max_steps):
        f = featurize(env.raw_observation())
        a, logp, v = policy.act(f, deterministic=deterministic, rng=rng)
        _, r, term, trunc, info = env.step(a)
        feats.append(f); acts.append(a); rews.append(r)
        logps.append(logp); vals.append(v)
        ret += r
        success = success or bool(info.get("is_success"))
        if term or trunc:
            break
    return {
        "features": feats,
        "actions": np.stack(acts),
        "rewards": np.array(rews, dtype=np.float64),
        "logps": np.array(logps, dtype=np.float64),
        "values": np.array(vals, dtype=np.float64),
        "ret": ret,
        "success": success,
    }


def returns_to_go(rewards: np.ndarray, gamma: float) -> np.ndarray:
    out = np.zeros_like(rewards)
    acc = 0.0
    for i in range(len(rewards) - 1, -1, -1):
        acc = rewards[i] + gamma * acc
        out[i] = acc
    return out
