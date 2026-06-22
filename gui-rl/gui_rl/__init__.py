"""gui_rl — a from-scratch reinforcement-learning framework for Windows GUI agents.

The package follows Gymnasium + CleanRL conventions so it is familiar to RL
practitioners, while staying honest about the realities of GUI control:

* Observations are *pixels + an accessibility (UI Automation) tree*.
* Actions are *grounded* GUI verbs (click an element / coordinate, type, key,
  scroll, drag, finish) rather than raw motor outputs.
* Training is *offline-first*: behavior-clone from collected trajectories, then
  RL-fine-tune the policy (GRPO / PPO). This matches what actually moves the
  state of the art for GUI agents — you cannot afford millions of live desktop
  steps, so you bootstrap from logged trajectories and refine with RL.

The environment talks to a pluggable :class:`~gui_rl.backends.base.GuiBackend`:

* :class:`~gui_rl.backends.mock.MockGuiBackend` — a deterministic simulated
  desktop that runs anywhere (CI, this container) so the whole pipeline is
  exercisable without Windows or a GPU.
* :class:`~gui_rl.backends.windows_uia.WindowsUiaBackend` — the real thing,
  driving a Windows desktop via UI Automation + screen capture. Stubbed so it
  imports cleanly on non-Windows hosts and tells you exactly what to install.
"""

from gui_rl.actions import VERBS, GuiAction
from gui_rl.env import WindowsGuiEnv

__all__ = ["GuiAction", "VERBS", "WindowsGuiEnv", "__version__"]

__version__ = "0.1.0"
