"""Command-line entrypoints: collect / bc / rlft / eval.

    python -m gui_rl.cli collect --task enable_dark_mode --episodes 200 --out data/expert.jsonl
    python -m gui_rl.cli bc      --data data/expert.jsonl --out runs/bc.npz
    python -m gui_rl.cli rlft    --algo grpo --init runs/bc.npz --out runs/rlft.npz
    python -m gui_rl.cli eval    --policy runs/rlft.npz
"""

from __future__ import annotations

import argparse

from gui_rl.algos.bc import behavior_clone
from gui_rl.algos.grpo import grpo_finetune
from gui_rl.algos.ppo import ppo_finetune
from gui_rl.data import TrajectoryDataset, collect, load_trajectories, save_trajectories
from gui_rl.env import WindowsGuiEnv
from gui_rl.evaluate import evaluate
from gui_rl.policies.linear import LinearGuiPolicy
from gui_rl.tasks import all_tasks, make_task


def _collect(a):
    tasks = [make_task(a.task)] if a.task else all_tasks()
    trajs = []
    for t in tasks:
        env = WindowsGuiEnv(t)
        trajs += collect(env, a.episodes)
    save_trajectories(trajs, a.out)
    print(f"wrote {len(trajs)} trajectories -> {a.out}")


def _bc(a):
    ds = TrajectoryDataset(load_trajectories(a.data))
    policy = LinearGuiPolicy(seed=a.seed)
    behavior_clone(policy, ds, epochs=a.epochs, lr=a.lr, seed=a.seed, verbose=True)
    policy.save(a.out)
    print(evaluate(policy))


def _rlft(a):
    policy = LinearGuiPolicy(seed=a.seed)
    if a.init:
        policy.load(a.init)
    tasks = all_tasks()
    fn = grpo_finetune if a.algo == "grpo" else ppo_finetune
    fn(policy, tasks, iterations=a.iterations, repair_a11y=not a.no_repair,
       seed=a.seed, verbose=True)
    policy.save(a.out)
    print(evaluate(policy, repair_a11y=not a.no_repair))


def _eval(a):
    policy = LinearGuiPolicy()
    policy.load(a.policy)
    print("with instrumentation (a11y repaired):", evaluate(policy, repair_a11y=True))
    print("without instrumentation (raw UIA)   :", evaluate(policy, repair_a11y=False))


def main(argv=None):
    p = argparse.ArgumentParser(prog="gui_rl")
    sub = p.add_subparsers(dest="cmd", required=True)

    c = sub.add_parser("collect"); c.set_defaults(fn=_collect)
    c.add_argument("--task"); c.add_argument("--episodes", type=int, default=200)
    c.add_argument("--out", default="data/expert.jsonl")

    b = sub.add_parser("bc"); b.set_defaults(fn=_bc)
    b.add_argument("--data", default="data/expert.jsonl")
    b.add_argument("--out", default="runs/bc.npz")
    b.add_argument("--epochs", type=int, default=30); b.add_argument("--lr", type=float, default=5e-2)
    b.add_argument("--seed", type=int, default=0)

    r = sub.add_parser("rlft"); r.set_defaults(fn=_rlft)
    r.add_argument("--algo", choices=["grpo", "ppo"], default="grpo")
    r.add_argument("--init"); r.add_argument("--out", default="runs/rlft.npz")
    r.add_argument("--iterations", type=int, default=60)
    r.add_argument("--no-repair", action="store_true", help="ablate instrumentation")
    r.add_argument("--seed", type=int, default=0)

    e = sub.add_parser("eval"); e.set_defaults(fn=_eval)
    e.add_argument("--policy", default="runs/rlft.npz")

    args = p.parse_args(argv)
    args.fn(args)


if __name__ == "__main__":
    main()
