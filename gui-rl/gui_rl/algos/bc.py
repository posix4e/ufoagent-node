"""Behavior cloning — the offline bootstrap.

Supervised imitation of logged (expert or production) trajectories. This is step
one of the offline-first recipe: get a competent policy from data you already
have, *then* RL-fine-tune it. Cloning first keeps RL stable — fine-tuning a
random policy on sparse GUI reward rarely takes off.
"""

from __future__ import annotations

import numpy as np

from gui_rl.data import TrajectoryDataset
from gui_rl.policies.base import Adam
from gui_rl.policies.linear import LinearGuiPolicy, stack_features


def behavior_clone(
    policy: LinearGuiPolicy,
    dataset: TrajectoryDataset,
    epochs: int = 30,
    batch_size: int = 64,
    lr: float = 5e-2,
    seed: int = 0,
    verbose: bool = False,
) -> list[dict]:
    opt = Adam(policy.p, lr=lr)
    rng = np.random.default_rng(seed)
    history = []
    for ep in range(epochs):
        losses, correct, total = [], 0, 0
        for batch in dataset.batches(batch_size, shuffle=True, rng=rng):
            feats = stack_features([b["features"] for b in batch])
            actions = np.stack([b["action"] for b in batch])
            o = policy.forward(feats)
            grads, loss = policy.bc_grads(o, actions)
            opt.step(grads)
            losses.append(loss)
            pred_v = o.pv.argmax(1)
            correct += int((pred_v == actions[:, 0]).sum())
            total += len(actions)
        m = {"epoch": ep, "loss": float(np.mean(losses)),
             "verb_acc": correct / max(total, 1)}
        history.append(m)
        if verbose:
            print(f"[bc] epoch {ep:3d}  loss {m['loss']:.4f}  verb_acc {m['verb_acc']:.3f}")
    return history
