"""``LinearGuiPolicy`` — a factored linear-softmax grounded GUI policy (numpy).

Three heads over the MultiDiscrete action ``[verb, element, param]``:

* **verb** and **param** heads read a global state vector ``S`` =
  ``[instruction ⊕ mean-pool(elements) ⊕ max-pool(elements)]``.
* the **element** head scores each control from
  ``[element_features ⊕ instruction ⊕ (element-name-hash ⊙ instruction)]`` — the
  product term is the grounding signal that lets a linear model click the control
  whose name matches the instruction. Scores are masked to real controls.

All gradients are analytic (linear → softmax → cross-entropy), so behavior
cloning and policy-gradient fine-tuning run on CPU with no autodiff. This is the
*reference* policy that exercises the whole pipeline; swap in
:mod:`gui_rl.policies.vlm` for a pixels+a11y VLM on real hardware.
"""

from __future__ import annotations

import numpy as np

from gui_rl import actions as A
from gui_rl.backends.base import CONTROL_TYPES
from gui_rl.features import ELEM_FEAT, HASH_DIM, MAX_ELEMENTS
from gui_rl.policies.base import Policy, softmax

_NAME0 = len(CONTROL_TYPES) + 4 + 4          # offset of name-hash inside elem feat
_NAME1 = _NAME0 + HASH_DIM
DS = HASH_DIM + 2 * ELEM_FEAT                 # global state vector width
DE = ELEM_FEAT + 2 * HASH_DIM                 # per-element scoring input width

_ELEMENT_VERB_IDS = np.array([A.VERB_INDEX[v] for v in A.ELEMENT_VERBS])
_PARAM_VERB_IDS = np.array([A.VERB_INDEX[v] for v in A.PARAM_VERBS])


def stack_features(batch: list[dict]) -> dict:
    return {
        "elements": np.stack([b["elements"] for b in batch]),
        "element_mask": np.stack([b["element_mask"] for b in batch]).astype(np.float32),
        "instruction": np.stack([b["instruction"] for b in batch]).astype(np.float32),
    }


class Out:
    """Cached forward pass for backprop."""

    __slots__ = ("S", "E_in", "mask", "pv", "pe", "pp", "value",
                 "logits_v", "logits_e", "logits_p")


class LinearGuiPolicy(Policy):
    def __init__(self, seed: int = 0):
        rng = np.random.default_rng(seed)
        scale = 0.01
        self.p = {
            "Wv": rng.normal(0, scale, (DS, A.NUM_VERBS)),
            "bv": np.zeros(A.NUM_VERBS),
            "Wp": rng.normal(0, scale, (DS, A.NUM_PARAMS)),
            "bp": np.zeros(A.NUM_PARAMS),
            "we": rng.normal(0, scale, (DE,)),
            "be": np.zeros(1),
            "Wval": rng.normal(0, scale, (DS, 1)),
            "bval": np.zeros(1),
        }

    # -- feature assembly -------------------------------------------------
    def _assemble(self, feats: dict):
        elements = feats["elements"]                 # (B, M, ELEM_FEAT)
        mask = feats["element_mask"]                 # (B, M)
        instr = feats["instruction"]                 # (B, HASH_DIM)
        b = elements.shape[0]
        m3 = mask[..., None]
        denom = np.clip(mask.sum(1, keepdims=True), 1, None)
        mean_pool = (elements * m3).sum(1) / denom
        masked = np.where(m3 > 0, elements, -1e9)
        max_pool = masked.max(1)
        max_pool = np.where(np.isfinite(max_pool), max_pool, 0.0)
        S = np.concatenate([instr, mean_pool, max_pool], axis=1)   # (B, DS)

        name_hash = elements[:, :, _NAME0:_NAME1]                  # (B, M, HASH_DIM)
        instr_b = np.broadcast_to(instr[:, None, :], name_hash.shape)
        prod = name_hash * instr_b
        E_in = np.concatenate([elements, instr_b, prod], axis=2)   # (B, M, DE)
        return S, E_in, mask

    def forward(self, feats: dict) -> Out:
        S, E_in, mask = self._assemble(feats)
        o = Out()
        o.S, o.E_in, o.mask = S, E_in, mask
        o.logits_v = S @ self.p["Wv"] + self.p["bv"]
        o.logits_p = S @ self.p["Wp"] + self.p["bp"]
        o.value = (S @ self.p["Wval"] + self.p["bval"])[:, 0]
        elem_scores = E_in @ self.p["we"] + self.p["be"]          # (B, M)
        o.logits_e = np.where(mask > 0, elem_scores, -1e9)
        o.pv = softmax(o.logits_v)
        o.pe = softmax(o.logits_e)
        o.pp = softmax(o.logits_p)
        return o

    # -- sampling ---------------------------------------------------------
    def act(self, features: dict, deterministic: bool = False, rng=None):
        o = self.forward(stack_features([features]))
        rng = rng or np.random
        if deterministic:
            v, e, pr = int(o.pv[0].argmax()), int(o.pe[0].argmax()), int(o.pp[0].argmax())
        else:
            v = int(rng.choice(A.NUM_VERBS, p=o.pv[0]))
            e = int(rng.choice(MAX_ELEMENTS, p=o.pe[0]))
            pr = int(rng.choice(A.NUM_PARAMS, p=o.pp[0]))
        action = np.array([v, e, pr], dtype=np.int64)
        logp = float(np.log(o.pv[0, v] + 1e-12) + np.log(o.pe[0, e] + 1e-12)
                     + np.log(o.pp[0, pr] + 1e-12))
        return action, logp, float(o.value[0])

    # -- log-prob / entropy ----------------------------------------------
    @staticmethod
    def _gather(p, idx):
        return p[np.arange(len(idx)), idx]

    def logp_entropy(self, o: Out, actions: np.ndarray):
        v, e, pr = actions[:, 0], actions[:, 1], actions[:, 2]
        logp = (np.log(self._gather(o.pv, v) + 1e-12)
                + np.log(self._gather(o.pe, e) + 1e-12)
                + np.log(self._gather(o.pp, pr) + 1e-12))
        ent = (-(o.pv * np.log(o.pv + 1e-12)).sum(1)
               - (o.pe * np.log(o.pe + 1e-12)).sum(1)
               - (o.pp * np.log(o.pp + 1e-12)).sum(1))
        return logp, ent

    # -- gradient builders ------------------------------------------------
    def _zero_grads(self) -> dict:
        return {k: np.zeros_like(v) for k, v in self.p.items()}

    def _accumulate_head_grads(self, g, o, dlv, dle, dlp, dval):
        """Backprop dlogits/dvalue through the linear heads into param grads."""
        B = o.S.shape[0]
        g["Wv"] += o.S.T @ dlv
        g["bv"] += dlv.sum(0)
        g["Wp"] += o.S.T @ dlp
        g["bp"] += dlp.sum(0)
        g["Wval"] += o.S.T @ dval[:, None]
        g["bval"] += dval.sum()
        # element head: scalar weight over (B, M, DE), gated by mask
        gated = dle * (o.mask > 0)
        g["we"] += (o.E_in * gated[..., None]).reshape(-1, o.E_in.shape[-1]).sum(0)
        g["be"] += gated.sum()
        return B

    def bc_grads(self, o: Out, actions: np.ndarray):
        """Behavior-cloning cross-entropy grads. Element/param losses apply only
        to verbs that use those slots."""
        B = o.S.shape[0]
        v, e, pr = actions[:, 0], actions[:, 1], actions[:, 2]
        wv = np.isin(v, _ELEMENT_VERB_IDS).astype(np.float32)   # element-loss mask
        wp = np.isin(v, _PARAM_VERB_IDS).astype(np.float32)     # param-loss mask

        dlv = o.pv.copy(); dlv[np.arange(B), v] -= 1.0; dlv /= B
        dle = o.pe.copy(); dle[np.arange(B), e] -= 1.0; dle *= (wv / B)[:, None]
        dlp = o.pp.copy(); dlp[np.arange(B), pr] -= 1.0; dlp *= (wp / B)[:, None]

        g = self._zero_grads()
        self._accumulate_head_grads(g, o, dlv, dle, dlp, np.zeros(B))
        loss = -(np.log(self._gather(o.pv, v) + 1e-12).mean()
                 + (np.log(self._gather(o.pe, e) + 1e-12) * wv).sum() / max(wv.sum(), 1)
                 + (np.log(self._gather(o.pp, pr) + 1e-12) * wp).sum() / max(wp.sum(), 1))
        return g, float(loss)

    def pg_grads(self, o: Out, actions: np.ndarray, adv: np.ndarray,
                 ent_coef: float = 0.0):
        """REINFORCE / GRPO policy-gradient grads (no critic).

        loss = -mean(adv * logp) - ent_coef * mean(entropy).
        """
        B = o.S.shape[0]
        v, e, pr = actions[:, 0], actions[:, 1], actions[:, 2]
        a = adv.astype(np.float64)

        def head_grad(p, idx):
            d = p.copy()
            d[np.arange(B), idx] -= 1.0          # d(-logp)/dlogits = p - onehot
            return d * (a / B)[:, None]
        dlv = head_grad(o.pv, v)
        dle = head_grad(o.pe, e)
        dlp = head_grad(o.pp, pr)
        if ent_coef:
            dlv += ent_coef * self._entropy_dlogits(o.pv) / B
            dle += ent_coef * self._entropy_dlogits(o.pe) / B
            dlp += ent_coef * self._entropy_dlogits(o.pp) / B
        g = self._zero_grads()
        self._accumulate_head_grads(g, o, dlv, dle, dlp, np.zeros(B))
        return g

    def ppo_grads(self, o: Out, actions: np.ndarray, old_logp: np.ndarray,
                  adv: np.ndarray, returns: np.ndarray, clip: float = 0.2,
                  vf_coef: float = 0.5, ent_coef: float = 0.0):
        """Clipped PPO surrogate + value MSE grads."""
        B = o.S.shape[0]
        logp, _ = self.logp_entropy(o, actions)
        ratio = np.exp(np.clip(logp - old_logp, -20, 20))
        unclipped = ratio * adv
        clipped = np.clip(ratio, 1 - clip, 1 + clip) * adv
        use_unclipped = unclipped <= clipped               # min() picks this branch
        # d(-surrogate)/dlogp = -adv*ratio where unclipped active & not frozen
        active = use_unclipped | ((ratio < 1 + clip) & (ratio > 1 - clip))
        coef = -(adv * ratio) * active / B                 # (B,)

        v, e, pr = actions[:, 0], actions[:, 1], actions[:, 2]

        def head_grad(p, idx):
            d = -p.copy()
            d[np.arange(B), idx] += 1.0                     # dlogp/dlogits = onehot - p
            return d * coef[:, None]
        dlv = head_grad(o.pv, v)
        dle = head_grad(o.pe, e)
        dlp = head_grad(o.pp, pr)
        if ent_coef:
            dlv += ent_coef * self._entropy_dlogits(o.pv) / B
            dle += ent_coef * self._entropy_dlogits(o.pe) / B
            dlp += ent_coef * self._entropy_dlogits(o.pp) / B
        dval = vf_coef * 2.0 * (o.value - returns) / B      # MSE grad
        g = self._zero_grads()
        self._accumulate_head_grads(g, o, dlv, dle, dlp, dval)
        return g

    @staticmethod
    def _entropy_dlogits(p):
        """d(-entropy)/dlogits = p * (log p + H)  (we minimise -entropy)."""
        H = -(p * np.log(p + 1e-12)).sum(1, keepdims=True)
        return p * (np.log(p + 1e-12) + H)

    # -- persistence ------------------------------------------------------
    def save(self, path: str) -> None:
        np.savez(path, **self.p)

    def load(self, path: str) -> None:
        data = np.load(path if path.endswith(".npz") else path + ".npz")
        self.p = {k: data[k] for k in data.files}
