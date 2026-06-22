"""A deterministic simulated desktop *with simulated instrumentation*.

The mock backend is a small **screen graph**: each screen exposes a set of
controls, and transitions fire when the agent performs a matching
``(verb, element, param)``. State flags (``dark_mode``, ``saved``, ...) make
goals expressible. This turns GUI control into a genuine sequential decision
problem — multi-step plans, distractor controls, irreversible-ish actions — so
behavior cloning and RL fine-tuning have something real to learn, running
anywhere with no Windows and no GPU.

It also models the two faces of perception that motivate dynamic instrumentation:

* The **screenshot** is rasterised from *every drawn control* — pixels always
  show what's painted.
* The **accessibility tree** can be *degraded*: a control may be invisible to
  UIA (``a11y_visible=False``, e.g. custom-drawn) or mislabeled
  (``a11y_name``). With instrumentation enabled (``repair_a11y=True``) those
  controls are recovered and the policy sees the true tree; with it disabled the
  policy is left with the lossy UIA view. The difference is the value of
  instrumentation, measurable by ablation.

``signals`` (ground-truth state) and ``events`` (what fired in-process) are the
privileged instrumentation outputs used by the reward/success oracle.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Callable, Optional

import numpy as np

from gui_rl.backends.base import Element, GuiBackend, Observation

# Window messages a click/type/key would dispatch — surfaced in the event log to
# mirror what a real hook (SendMessage/PostMessage interception) would report.
_VERB_MESSAGE = {
    "click": "BM_CLICK",
    "double_click": "WM_LBUTTONDBLCLK",
    "right_click": "WM_CONTEXTMENU",
    "type": "WM_SETTEXT",
    "key": "WM_KEYDOWN",
    "scroll": "WM_MOUSEWHEEL",
    "drag": "WM_MOUSEMOVE",
}


@dataclass
class ElementSpec:
    name: str
    control_type: str = "Button"
    checked_key: Optional[str] = None      # state key that drives `checked`
    enabled: bool = True
    a11y_visible: bool = True              # does UIA expose this control at all?
    a11y_name: Optional[str] = None       # name UIA reports (None = same as `name`)


@dataclass
class Screen:
    name: str
    elements: list[ElementSpec]


@dataclass
class Transition:
    screen: str
    verb: str
    element: Optional[str] = None      # *true* element name to match
    param: Optional[str] = None
    to_screen: Optional[str] = None
    set_state: dict = field(default_factory=dict)


@dataclass
class Scenario:
    instruction: str
    start_screen: str
    screens: dict[str, Screen]
    transitions: list[Transition]
    init_state: dict = field(default_factory=dict)


class MockGuiBackend(GuiBackend):
    """Drives a :class:`Scenario`.

    Args:
        scenario_factory: builds a (possibly seed-varying) scenario per reset.
        repair_a11y: when True, instrumentation recovers a11y-invisible /
            mislabeled controls so the policy sees the true tree. Set False to
            ablate the value of instrumentation.
    """

    def __init__(
        self,
        scenario_factory: Callable[[Optional[int]], Scenario],
        repair_a11y: bool = True,
    ):
        self._factory = scenario_factory
        self.repair_a11y = repair_a11y
        self._scenario: Optional[Scenario] = None
        self._screen: str = ""
        self._state: dict = {}
        self._last_events: list[dict] = []

    # -- GuiBackend API ---------------------------------------------------
    def reset(self, instruction: str, seed: Optional[int] = None) -> Observation:
        self._scenario = self._factory(seed)
        self._screen = self._scenario.start_screen
        self._state = dict(self._scenario.init_state)
        self._last_events = []
        return self.observe()

    def observe(self) -> Observation:
        visible, _ = self._a11y_view()
        return Observation(
            screenshot=self._render(self._true_elements()),
            elements=visible,
            instruction=self._scenario.instruction,
            signals={**self._state, "screen": self._screen},   # privileged oracle
            events=list(self._last_events),                      # privileged events
            extra={"repair_a11y": self.repair_a11y},
        )

    def execute(self, action) -> dict:
        visible, truename = self._a11y_view()
        element_name = None
        if action.element_id is not None and 0 <= action.element_id < len(visible):
            element_name = truename[action.element_id]   # act on the true control

        events: list[dict] = []
        for t in self._scenario.transitions:
            if t.screen != self._screen or t.verb != action.verb:
                continue
            if t.element is not None and t.element != element_name:
                continue
            if t.param is not None and t.param != action.param:
                continue
            if t.to_screen is not None:
                self._screen = t.to_screen
            self._state.update(t.set_state)
            events.append({
                "kind": "window_message",
                "message": _VERB_MESSAGE.get(action.verb, "WM_NULL"),
                "control": element_name,
            })
            if t.set_state:
                events.append({"kind": "state_delta", "set": dict(t.set_state)})
            self._last_events = events
            return {"ok": True, "no_op": False, "element": element_name}

        # No transition matched: a wasted action on a distractor / dead control.
        self._last_events = [
            {"kind": "ignored", "verb": action.verb, "control": element_name}
        ]
        return {"ok": True, "no_op": True, "element": element_name}

    # -- internals --------------------------------------------------------
    def _true_elements(self) -> list[tuple[ElementSpec, Element]]:
        screen = self._scenario.screens[self._screen]
        out: list[tuple[ElementSpec, Element]] = []
        n = max(len(screen.elements), 1)
        for i, spec in enumerate(screen.elements):
            checked = bool(self._state.get(spec.checked_key)) if spec.checked_key else False
            y = (i + 0.5) / n
            el = Element(
                name=spec.name,
                control_type=spec.control_type,
                bbox=(0.1, max(0.0, y - 0.06), 0.8, 0.12),
                enabled=spec.enabled,
                focused=(i == 0),
                checked=checked,
                automation_id=f"{self._screen}/{i}",
            )
            out.append((spec, el))
        return out

    def _a11y_view(self) -> tuple[list[Element], list[str]]:
        """Policy-visible accessibility tree + parallel list of *true* names.

        With ``repair_a11y`` the instrumentation recovers every control with its
        true name; otherwise UIA-invisible controls are dropped and mislabeled
        ones keep their (possibly blank) UIA name.
        """
        visible: list[Element] = []
        truenames: list[str] = []
        for spec, el in self._true_elements():
            if self.repair_a11y:
                visible.append(el)
                truenames.append(spec.name)
                continue
            if not spec.a11y_visible:
                continue  # UIA never exposed it
            shown = el
            if spec.a11y_name is not None:
                shown = Element(**{**el.__dict__, "name": spec.a11y_name})
            visible.append(shown)
            truenames.append(spec.name)
        return visible, truenames

    def _render(self, elements: list[tuple[ElementSpec, Element]]) -> np.ndarray:
        h, w = self.screen_hw
        img = np.full((h, w, 3), 24, dtype=np.uint8)  # dark desktop background
        for _spec, el in elements:
            x0 = int(el.bbox[0] * w)
            y0 = int(el.bbox[1] * h)
            x1 = min(w, x0 + int(el.bbox[2] * w))
            y1 = min(h, y0 + int(el.bbox[3] * h))
            hue = (el.control_type_index() * 23) % 180
            base = 60 + (hue % 3) * 30
            if not el.enabled:
                base = 40
            if el.checked:
                base = 220
            if el.focused:
                base = min(255, base + 40)
            color = np.array([base, (base * 2) % 256, (hue * 3) % 256], dtype=np.uint8)
            img[y0:y1, x0:x1] = color
        return img
