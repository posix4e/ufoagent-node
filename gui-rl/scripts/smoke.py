"""End-to-end smoke run of the offline-first GUI RL pipeline (CPU, no Windows).

Demonstrates three things the framework is built to show:

1. **Offline-first works**: behavior-clone from logged trajectories, then RL
   fine-tune (GRPO) with the instrumentation reward oracle -> high success.
2. **Why offline-first**: pure online RL from scratch, at the *same* sample
   budget, barely moves — sparse GUI reward is brutal without a BC bootstrap.
3. **Why instrument the binary**: the same policy, evaluated with the a11y tree
   repaired by instrumentation vs. the raw (lossy) UIA view — the difference is
   the value of dynamic instrumentation, as a number.

Run:  python scripts/smoke.py
"""

from __future__ import annotations

from gui_rl.algos.bc import behavior_clone
from gui_rl.algos.grpo import grpo_finetune
from gui_rl.data import TrajectoryDataset, collect
from gui_rl.env import WindowsGuiEnv
from gui_rl.evaluate import evaluate
from gui_rl.policies.linear import LinearGuiPolicy
from gui_rl.tasks import all_tasks


def _mean(policy, **kw) -> float:
    return evaluate(policy, episodes=30, **kw)["mean"]


def main() -> None:
    tasks = all_tasks()
    print(f"tasks: {[t.name for t in tasks]}")

    # 1) Offline data from the scripted expert (stand-in for logged production runs).
    trajs = []
    for t in tasks:
        trajs += collect(WindowsGuiEnv(t), n_episodes=40)
    print(f"collected {len(trajs)} expert trajectories "
          f"(success {sum(tr.success for tr in trajs) / len(trajs):.2f})")

    # 2) Offline-first: light behavior cloning, then RL fine-tune (GRPO).
    policy = LinearGuiPolicy(seed=0)
    behavior_clone(policy, TrajectoryDataset(trajs), epochs=2, lr=5e-2, seed=0)
    bc_mean = _mean(policy)
    grpo_finetune(policy, tasks, iterations=150, group_size=10, lr=3e-2,
                  ent_coef=0.01, seed=0)
    rl_mean = _mean(policy)

    # 3) Contrast: pure online RL from scratch at the same budget.
    scratch = LinearGuiPolicy(seed=0)
    grpo_finetune(scratch, tasks, iterations=150, group_size=10, lr=3e-2,
                  ent_coef=0.01, seed=0)
    scratch_mean = _mean(scratch)

    # 4) Ablation: value of dynamic instrumentation (a11y repair on vs off).
    on = _mean(policy, repair_a11y=True)
    off = _mean(policy, repair_a11y=False)

    print("\n=== mean success over task suite ===")
    print(f"  light behavior cloning      : {bc_mean:.2f}")
    print(f"  BC + GRPO (offline-first)   : {rl_mean:.2f}")
    print(f"  GRPO from scratch (same budget): {scratch_mean:.2f}")
    print("\n=== instrumentation ablation (final policy) ===")
    print(f"  a11y repaired by instrumentation : {on:.2f}")
    print(f"  raw UIA only (no instrumentation): {off:.2f}")
    print(f"  lift from instrumentation        : {on - off:+.2f}")


if __name__ == "__main__":
    main()
