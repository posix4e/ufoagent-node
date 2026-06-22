"""Real Windows backend: UI Automation accessibility tree + screen capture.

This is the production adapter. It is intentionally a thin, well-documented stub
so the package imports on any OS; the heavy, Windows-only dependencies are
imported lazily inside :meth:`WindowsUiaBackend.reset` so you get an actionable
error (with the exact ``pip install``) instead of an import explosion.

Wiring it up on a real machine:

* **Accessibility tree** — ``uiautomation`` (or ``pywinauto``) walks the UIA tree.
  Map each control to :class:`~gui_rl.backends.base.Element` (name, ControlType,
  normalised BoundingRectangle, IsEnabled, HasKeyboardFocus, Toggle/Selection
  state). Filter to actionable, on-screen controls and cap at ``max_elements``.
* **Screenshot** — ``mss`` grabs the primary monitor; downscale to
  :attr:`screen_hw`.
* **Execute** — translate a :class:`~gui_rl.actions.GuiAction` into UIA invokes
  (``Invoke``/``Toggle`` patterns) or ``SendInput`` mouse/keyboard. Prefer UIA
  patterns over raw coordinates when an element is targeted; they are far more
  robust to DPI and layout shifts.

The control plane in this repo already harvests UFO2 trajectories in a
compatible shape (see ``../src/trajectory.rs``), so logged production runs can be
replayed straight into :mod:`gui_rl.data` for offline training.
"""

from __future__ import annotations

from typing import Optional

from gui_rl.backends.base import GuiBackend, Observation

_INSTALL_HINT = (
    "WindowsUiaBackend needs a Windows host with: "
    "pip install 'gui-rl[windows]'  (uiautomation, pywinauto, mss, pillow). "
    "On Linux/macOS use MockGuiBackend for development and CI."
)


class WindowsUiaBackend(GuiBackend):
    def __init__(self, max_elements: int = 32, screen_hw: tuple[int, int] = (84, 84)):
        self.max_elements = max_elements
        self.screen_hw = screen_hw
        self._uia = None
        self._mss = None

    def _ensure_deps(self) -> None:
        if self._uia is not None:
            return
        try:  # pragma: no cover - exercised only on Windows
            import uiautomation as uia  # type: ignore
            import mss  # type: ignore
        except Exception as e:  # pragma: no cover
            raise RuntimeError(_INSTALL_HINT) from e
        self._uia = uia
        self._mss = mss

    def reset(self, instruction: str, seed: Optional[int] = None) -> Observation:  # pragma: no cover
        self._ensure_deps()
        # A real reset would bring the target app to a known state (launch / focus
        # / close dialogs). Left to the task harness on the deployment machine.
        return self.observe()

    def observe(self) -> Observation:  # pragma: no cover - Windows only
        self._ensure_deps()
        raise NotImplementedError(
            "Implement UIA tree walk + screen grab here. See module docstring."
        )

    def execute(self, action) -> dict:  # pragma: no cover - Windows only
        self._ensure_deps()
        raise NotImplementedError(
            "Map GuiAction -> UIA Invoke/Toggle or SendInput here."
        )
