"""Policy interface + a tiny Adam optimizer.

A policy maps featurised observations to a *factored* distribution over the
MultiDiscrete action ``[verb, element, param]``. Factoring lets a small model
ground actions over a variable-length accessibility tree: a shared per-element
head scores each control, while verb/param heads read pooled global features.
"""

from __future__ import annotations

import abc

import numpy as np


class Policy(abc.ABC):
    @abc.abstractmethod
    def act(self, features: dict, deterministic: bool = False):
        """Return ``(action_indices, log_prob, value)`` for one observation."""

    @abc.abstractmethod
    def save(self, path: str) -> None: ...

    @abc.abstractmethod
    def load(self, path: str) -> None: ...


class Adam:
    """Minimal Adam over a dict of named numpy parameter arrays."""

    def __init__(self, params: dict, lr: float = 1e-2, betas=(0.9, 0.999), eps: float = 1e-8):
        self.params = params
        self.lr = lr
        self.b1, self.b2 = betas
        self.eps = eps
        self.m = {k: np.zeros_like(v) for k, v in params.items()}
        self.v = {k: np.zeros_like(v) for k, v in params.items()}
        self.t = 0

    def step(self, grads: dict) -> None:
        self.t += 1
        for k, g in grads.items():
            self.m[k] = self.b1 * self.m[k] + (1 - self.b1) * g
            self.v[k] = self.b2 * self.v[k] + (1 - self.b2) * (g * g)
            mhat = self.m[k] / (1 - self.b1 ** self.t)
            vhat = self.v[k] / (1 - self.b2 ** self.t)
            self.params[k] -= self.lr * mhat / (np.sqrt(vhat) + self.eps)


def softmax(z: np.ndarray, axis: int = -1) -> np.ndarray:
    z = z - z.max(axis=axis, keepdims=True)
    e = np.exp(z)
    return e / e.sum(axis=axis, keepdims=True)
