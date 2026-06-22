"""Trajectory schema, JSONL persistence, dataset, and a rollout collector.

The on-disk format deliberately mirrors the records the production node already
harvests from UFO2 (see ``../src/trajectory.rs``: round/step, agent thought,
action, control text, screenshot key). That means real, logged production runs
can be dropped straight into this offline pipeline — which is the whole point of
*offline RL*: learn from the trajectories you already have before you ever run
the policy live.

Each step keeps both human-readable fields (instruction, a11y controls, decoded
action) and the privileged instrumentation channels (signals, events) used by
the reward oracle. The :class:`TrajectoryDataset` re-featurises stored steps into
model inputs via :mod:`gui_rl.features`.
"""

from __future__ import annotations

import json
from dataclasses import asdict, dataclass, field
from pathlib import Path
from typing import Callable, Iterator, Optional

import numpy as np

from gui_rl.backends.base import Element, Observation
from gui_rl.features import featurize


@dataclass
class Step:
    round: int
    step: int
    instruction: str
    elements: list[dict]            # policy-visible a11y controls (re-featurisable)
    action: list[int]               # [verb, element, param] indices
    action_decoded: dict            # human-readable GuiAction
    reward: float
    done: bool
    signals: dict = field(default_factory=dict)   # privileged oracle state
    events: list = field(default_factory=list)     # privileged instrumentation events


@dataclass
class Trajectory:
    task: str
    steps: list[Step]
    success: bool
    ret: float                       # undiscounted episode return

    def to_jsonl(self) -> str:
        head = {"type": "trajectory", "task": self.task,
                "success": self.success, "return": self.ret}
        lines = [json.dumps(head)]
        lines += [json.dumps({"type": "step", **asdict(s)}) for s in self.steps]
        return "\n".join(lines)


def save_trajectories(trajs: list[Trajectory], path: str | Path) -> None:
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w") as f:
        for t in trajs:
            f.write(t.to_jsonl() + "\n")


def load_trajectories(path: str | Path) -> list[Trajectory]:
    trajs: list[Trajectory] = []
    cur: Optional[dict] = None
    steps: list[Step] = []
    for line in Path(path).read_text().splitlines():
        line = line.strip()
        if not line:
            continue
        rec = json.loads(line)
        if rec.get("type") == "trajectory":
            if cur is not None:
                trajs.append(_finish(cur, steps))
            cur, steps = rec, []
        elif rec.get("type") == "step":
            rec.pop("type")
            steps.append(Step(**rec))
    if cur is not None:
        trajs.append(_finish(cur, steps))
    return trajs


def _finish(head: dict, steps: list[Step]) -> Trajectory:
    return Trajectory(task=head["task"], steps=steps,
                      success=head["success"], ret=head["return"])


def _element_from_dict(d: dict) -> Element:
    return Element(
        name=d.get("name", ""),
        control_type=d.get("control_type", "Unknown"),
        bbox=tuple(d.get("bbox", (0, 0, 0, 0))),
        enabled=d.get("enabled", True),
        focused=d.get("focused", False),
        checked=d.get("checked", False),
        selected=d.get("selected", False),
        automation_id=d.get("automation_id", ""),
    )


def step_features(step: Step, screen_hw=(84, 84)) -> dict:
    """Re-featurise a stored step into policy inputs (zero screenshot; the
    tabular policy ignores pixels, a VLM policy would store/reload them)."""
    obs = Observation(
        screenshot=np.zeros((*screen_hw, 3), dtype=np.uint8),
        elements=[_element_from_dict(e) for e in step.elements],
        instruction=step.instruction,
    )
    return featurize(obs)


class TrajectoryDataset:
    """Flat (features, action, reward, return-to-go) view over trajectories."""

    def __init__(self, trajs: list[Trajectory], gamma: float = 0.99):
        self.samples: list[dict] = []
        for t in trajs:
            rtg = 0.0
            acc: list[dict] = []
            for s in reversed(t.steps):
                rtg = s.reward + gamma * rtg
                acc.append({
                    "features": step_features(s),
                    "action": np.array(s.action, dtype=np.int64),
                    "reward": s.reward,
                    "rtg": rtg,
                    "success": t.success,
                })
            self.samples.extend(reversed(acc))

    def __len__(self) -> int:
        return len(self.samples)

    def __iter__(self) -> Iterator[dict]:
        return iter(self.samples)

    def batches(self, batch_size: int, shuffle: bool = True, rng=None):
        idx = np.arange(len(self.samples))
        if shuffle:
            (rng or np.random).shuffle(idx)
        for i in range(0, len(idx), batch_size):
            yield [self.samples[j] for j in idx[i:i + batch_size]]


# -- rollout collection --------------------------------------------------
def rollout(env, policy: Callable, max_steps: Optional[int] = None) -> Trajectory:
    """Run one episode. ``policy(env)`` returns MultiDiscrete action indices."""
    env.reset()
    steps: list[Step] = []
    ret = 0.0
    success = False
    limit = max_steps or env.task.max_steps
    for k in range(limit):
        raw = env.raw_observation()
        action = np.asarray(policy(env), dtype=np.int64)
        _, reward, term, trunc, info = env.step(action)
        ret += reward
        success = success or bool(info.get("is_success"))
        steps.append(Step(
            round=0,
            step=k,
            instruction=raw.instruction,
            elements=[_el_dict(e) for e in raw.elements],
            action=action.tolist(),
            action_decoded=info["action"],
            reward=float(reward),
            done=bool(term or trunc),
            signals=raw.signals,
            events=raw.events,
        ))
        if term or trunc:
            break
    return Trajectory(task=env.task.name, steps=steps, success=success, ret=ret)


def _el_dict(e: Element) -> dict:
    return {
        "name": e.name, "control_type": e.control_type, "bbox": list(e.bbox),
        "enabled": e.enabled, "focused": e.focused, "checked": e.checked,
        "selected": e.selected, "automation_id": e.automation_id,
    }


def expert_policy(env):
    return env.expert_action()


def random_policy(env):
    return env.action_space.sample()


def collect(env, n_episodes: int, policy: Callable = expert_policy) -> list[Trajectory]:
    return [rollout(env, policy) for _ in range(n_episodes)]
