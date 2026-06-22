"""Grounded GUI action space.

An action is a *verb* applied to (optionally) a *target element* with an
optional *parameter* (text to type, key chord, scroll direction). This mirrors
how production GUI agents such as Microsoft UFO2 act on the desktop: they select
a control from the accessibility tree and invoke an operation on it.

For RL we encode an action as three discrete indices
``[verb, element, param]`` (a :class:`gymnasium.spaces.MultiDiscrete`). The
environment decodes those indices against the *current* accessibility tree and
the shared parameter vocabulary into a concrete :class:`GuiAction`. Verbs that
do not need a target simply ignore the element index; the index is still part of
the action so the policy factorises cleanly and the log-probabilities stay
consistent for the policy-gradient update.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Optional

# Ordered verb vocabulary. Index = action id along the first MultiDiscrete axis.
VERBS = [
    "click",
    "double_click",
    "right_click",
    "type",
    "key",
    "scroll",
    "drag",
    "wait",
    "finish",
]
VERB_INDEX = {v: i for i, v in enumerate(VERBS)}

# Verbs that operate on a target element from the accessibility tree.
ELEMENT_VERBS = frozenset({"click", "double_click", "right_click", "type", "drag"})
# Verbs that consume the parameter slot (text / key chord / scroll direction).
PARAM_VERBS = frozenset({"type", "key", "scroll"})

# Shared parameter vocabulary. Kept small and task-relevant; a real deployment
# would learn text generation, but a fixed vocabulary keeps the action space
# discrete and the mock tasks solvable. Index 0 is the empty / no-op parameter.
PARAM_VOCAB = [
    "",            # 0: no parameter
    "enter",       # 1: key
    "tab",         # 2: key
    "esc",         # 3: key
    "ctrl+s",      # 4: key
    "ctrl+a",      # 5: key
    "up",          # 6: scroll / key
    "down",        # 7: scroll / key
    "hello world", # 8: type
    "ufoagent",    # 9: type
]
PARAM_INDEX = {p: i for i, p in enumerate(PARAM_VOCAB)}

NUM_VERBS = len(VERBS)
NUM_PARAMS = len(PARAM_VOCAB)


@dataclass
class GuiAction:
    """A decoded, backend-executable GUI action."""

    verb: str
    element_id: Optional[int] = None  # index into the current a11y element list
    param: str = ""                   # text / key chord / scroll direction
    meta: dict = field(default_factory=dict)

    def __post_init__(self) -> None:
        if self.verb not in VERB_INDEX:
            raise ValueError(f"unknown verb: {self.verb!r}")

    @property
    def needs_element(self) -> bool:
        return self.verb in ELEMENT_VERBS

    @property
    def needs_param(self) -> bool:
        return self.verb in PARAM_VERBS

    def to_indices(self) -> tuple[int, int, int]:
        """Encode back into ``[verb, element, param]`` indices."""
        return (
            VERB_INDEX[self.verb],
            int(self.element_id or 0),
            PARAM_INDEX.get(self.param, 0),
        )

    def to_json(self) -> dict:
        return {
            "verb": self.verb,
            "element_id": self.element_id,
            "param": self.param,
        }

    @classmethod
    def from_json(cls, d: dict) -> "GuiAction":
        return cls(
            verb=d["verb"],
            element_id=d.get("element_id"),
            param=d.get("param", "") or "",
        )

    def __str__(self) -> str:
        parts = [self.verb]
        if self.needs_element and self.element_id is not None:
            parts.append(f"#{self.element_id}")
        if self.needs_param and self.param:
            parts.append(repr(self.param))
        return "(" + " ".join(parts) + ")"


def decode(indices, elements) -> GuiAction:
    """Decode ``[verb, element, param]`` indices against the current a11y tree.

    ``elements`` is the list of accessibility elements visible this step; it is
    used only to clamp the element index into range so a policy can never select
    a non-existent control.
    """
    verb_i, elem_i, param_i = (int(x) for x in indices)
    verb = VERBS[verb_i % NUM_VERBS]
    param = PARAM_VOCAB[param_i % NUM_PARAMS]
    element_id: Optional[int] = None
    if verb in ELEMENT_VERBS and elements:
        element_id = elem_i % len(elements)
    return GuiAction(verb=verb, element_id=element_id, param=param if verb in PARAM_VERBS else "")
