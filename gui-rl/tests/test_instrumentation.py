"""The instrumentation contract: privileged signals/events + a11y repair."""

from gui_rl import actions as A
from gui_rl.backends.mock import MockGuiBackend
from gui_rl.env import WindowsGuiEnv
from gui_rl.evaluate import evaluate
from gui_rl.policies.linear import LinearGuiPolicy
from gui_rl.tasks import EnableDarkModeTask


def _backend(task, repair):
    return MockGuiBackend(lambda s: task.scenario(s), repair_a11y=repair)


def test_signals_are_ground_truth_oracle():
    t = EnableDarkModeTask()
    env = WindowsGuiEnv(t, backend=_backend(t, repair=True))
    env.reset(seed=0)
    assert env.raw_observation().signals.get("dark_mode") is False
    # Drive the expert to completion; signals must flip.
    for _ in range(t.max_steps):
        _, _, term, trunc, _ = env.step(env.expert_action())
        if term or trunc:
            break
    assert env.raw_observation().signals.get("dark_mode") is True


def test_events_log_window_messages():
    t = EnableDarkModeTask()
    env = WindowsGuiEnv(t, backend=_backend(t, repair=True))
    env.reset(seed=0)
    _, _, _, _, info = env.step(env.expert_action())  # click Start
    kinds = {e["kind"] for e in info["events"]}
    assert "window_message" in kinds


def test_a11y_repair_recovers_custom_drawn_control():
    t = EnableDarkModeTask()
    # Navigate to the personalization screen where the custom-drawn toggle lives.
    repaired = WindowsGuiEnv(t, backend=_backend(t, repair=True))
    raw = WindowsGuiEnv(t, backend=_backend(t, repair=False))
    for env in (repaired, raw):
        env.reset(seed=0)
        for name in ("Start", "Settings", "Personalization"):
            env.step([A.VERB_INDEX["click"],
                      _index(env, name), 0])
    repaired_names = {e.name for e in repaired.raw_observation().elements}
    raw_names = {e.name for e in raw.raw_observation().elements}
    assert "Dark Mode" in repaired_names      # instrumentation recovered it
    assert "Dark Mode" not in raw_names        # UIA never exposed it


def test_instrumentation_lifts_success():
    """Same trained policy should do no worse with repair than without."""
    t = EnableDarkModeTask()
    policy = LinearGuiPolicy(seed=0)
    # Hand the policy the solution via BC so the comparison is about perception.
    from gui_rl.algos.bc import behavior_clone
    from gui_rl.data import TrajectoryDataset, collect
    trajs = collect(WindowsGuiEnv(t), 60)
    behavior_clone(policy, TrajectoryDataset(trajs), epochs=20, seed=0)
    on = evaluate(policy, tasks=[t], episodes=20, repair_a11y=True)["mean"]
    off = evaluate(policy, tasks=[t], episodes=20, repair_a11y=False)["mean"]
    assert on > off


def _index(env, name):
    for i, e in enumerate(env.raw_observation().elements):
        if e.name == name:
            return i
    return 0
