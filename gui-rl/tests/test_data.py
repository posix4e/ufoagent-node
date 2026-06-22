from gui_rl.data import (
    TrajectoryDataset,
    collect,
    load_trajectories,
    save_trajectories,
)
from gui_rl.env import WindowsGuiEnv
from gui_rl.tasks import OpenCalculatorTask


def test_jsonl_roundtrip(tmp_path):
    env = WindowsGuiEnv(OpenCalculatorTask())
    trajs = collect(env, 5)
    path = tmp_path / "t.jsonl"
    save_trajectories(trajs, path)
    loaded = load_trajectories(path)
    assert len(loaded) == len(trajs)
    assert loaded[0].task == "open_calculator"
    assert loaded[0].steps[0].action == trajs[0].steps[0].action
    assert loaded[0].success == trajs[0].success


def test_dataset_refeaturizes_and_computes_rtg():
    env = WindowsGuiEnv(OpenCalculatorTask())
    ds = TrajectoryDataset(collect(env, 3), gamma=0.99)
    assert len(ds) > 0
    sample = next(iter(ds))
    assert set(sample["features"]) == {"screenshot", "elements", "element_mask", "instruction"}
    assert "rtg" in sample and "action" in sample
