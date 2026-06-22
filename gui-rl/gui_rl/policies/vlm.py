"""VLM grounded policy (torch) — interface stub.

The linear policy proves out the pipeline on CPU. The real-hardware policy is a
vision-language model that consumes the *screenshot* + tokenised *a11y tree* +
*instruction* and emits the grounded action. This module imports torch lazily so
the package stays installable without it.

A faithful implementation would:

* encode the screenshot with a vision backbone and the a11y/instruction text with
  the LM, fuse them, and produce the same factored heads
  (verb / element-pointer / param) used here — element selection as a pointer
  over encoded controls is the key grounding mechanism;
* expose ``act`` / ``logp_entropy`` / value head with identical signatures to
  :class:`~gui_rl.policies.linear.LinearGuiPolicy`, so :mod:`gui_rl.algos` (BC,
  GRPO, PPO) train it unchanged.

Bootstrap from an instruction-tuned VLM checkpoint, behavior-clone on logged
trajectories, then RL-fine-tune with the instrumentation reward oracle.
"""

from __future__ import annotations

_HINT = "VLMGuiPolicy needs torch + transformers: pip install 'gui-rl[torch]'."


class VLMGuiPolicy:  # pragma: no cover - requires torch + a GPU
    def __init__(self, *args, **kwargs):
        try:
            import torch  # type: ignore  # noqa: F401
        except Exception as e:
            raise RuntimeError(_HINT) from e
        raise NotImplementedError(
            "Implement the VLM encoder + factored grounded heads here. "
            "Keep act/logp_entropy/value signatures identical to LinearGuiPolicy."
        )
