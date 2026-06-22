"""Dynamic binary instrumentation: the privileged signal source.

The accessibility (UI Automation) tree is a lossy, often-incomplete view of a
Windows app. Worse, on a real desktop you usually have to *guess* whether an
action succeeded by scraping pixels — and noisy reward is what wrecks RL sample
efficiency. The fix is to **instrument the binary when we launch it** and read
ground truth straight from the process.

What an instrument provides:

* **signals** — a queryable ground-truth state of the app (toggles flipped,
  files/registry written, current view). This is the *reward / success oracle*.
* **events** — a structured log of what actually happened in-process since the
  last action (dispatched ``WM_COMMAND`` / ``BM_CLICK``, draw calls, syscalls).
  Great for credit assignment and verifiable trajectories.
* **a11y repair** — reconstruct controls UIA never exposed (custom-drawn,
  canvas, Electron, games) by observing ``CreateWindowEx`` / draw calls, then
  patch them back into the policy-visible accessibility tree.

These are *privileged*: used for reward and for repairing the observation, but
not handed to the policy as input. The deployed agent still runs on pixels+a11y.

Real Windows mechanisms (see :class:`FridaInstrument`): Frida or a Detours/
MinHook injected DLL for API hooking, ETW providers for low-overhead kernel/
user events, and DWM/Direct2D capture for the draw tree.
"""

from __future__ import annotations

import abc
from typing import Optional

from gui_rl.backends.base import Element


class Instrument(abc.ABC):
    """Attach to a launched binary and expose privileged signals."""

    enabled: bool = True

    @abc.abstractmethod
    def attach(self, target: str, seed: Optional[int] = None) -> None:
        """Launch/attach to ``target`` and install hooks."""

    @abc.abstractmethod
    def signals(self) -> dict:
        """Current ground-truth app state (the reward/success oracle)."""

    @abc.abstractmethod
    def drain_events(self) -> list[dict]:
        """Return structured in-process events since the previous call."""

    def repair(self, raw: list[Element]) -> list[Element]:
        """Augment a UIA tree with controls/metadata recovered from hooks.

        Default: identity. Overridden where the hook can reconstruct controls
        that UIA missed.
        """
        return raw

    def close(self) -> None:  # pragma: no cover - optional
        pass


class NullInstrument(Instrument):
    """No instrumentation: raw a11y only, no oracle. Models a vanilla deployment
    where you *cannot* hook the target — the hard, fully-from-pixels regime."""

    enabled = False

    def attach(self, target: str, seed: Optional[int] = None) -> None:
        pass

    def signals(self) -> dict:
        return {}

    def drain_events(self) -> list[dict]:
        return []


class FridaInstrument(Instrument):
    """Real Windows instrumentation (stub).

    Sketch of a production implementation::

        import frida
        self._session = frida.spawn(target); pid = ...; frida.resume(pid)
        script = session.create_script(JS_HOOKS)  # hook user32!SendMessageW,
                                                  # CreateWindowExW, gdi/d2d draws,
                                                  # advapi32!RegSetValueExW, ...
        script.on("message", self._on_event)      # -> self._events / self._signals

    ``signals()`` would expose derived state (e.g. a registry value the app just
    wrote that means "dark mode on"); ``repair()`` would inject windows seen via
    ``CreateWindowExW`` that never surfaced in UIA.
    """

    _HINT = (
        "FridaInstrument needs Windows + frida: pip install 'gui-rl[windows]'. "
        "Use the mock backend (which simulates instrumentation) off-Windows."
    )

    def __init__(self):
        self._session = None

    def attach(self, target: str, seed: Optional[int] = None) -> None:  # pragma: no cover
        try:
            import frida  # type: ignore  # noqa: F401
        except Exception as e:
            raise RuntimeError(self._HINT) from e
        raise NotImplementedError("Wire up frida.spawn + hook script here.")

    def signals(self) -> dict:  # pragma: no cover
        raise NotImplementedError

    def drain_events(self) -> list[dict]:  # pragma: no cover
        raise NotImplementedError
