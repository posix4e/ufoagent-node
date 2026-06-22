"""Turn an :class:`~gui_rl.backends.base.Observation` into fixed-shape arrays.

A grounded GUI policy has to connect *words in the instruction* to *names of
controls* in the accessibility tree. We hash both into the same-sized
bag-of-words space (the hashing trick) so their interaction is a simple
elementwise product — which lets even a linear policy learn "click the control
whose name matches the instruction". A VLM policy would instead consume the raw
screenshot + tokenised a11y text; the env exposes both.
"""

from __future__ import annotations

import zlib

import numpy as np

from gui_rl.backends.base import CONTROL_TYPES, Observation

MAX_ELEMENTS = 16
HASH_DIM = 16            # shared dim for instruction + element-name hashing
NUM_FLAGS = 4           # enabled, focused, checked, selected
ELEM_FEAT = len(CONTROL_TYPES) + 4 + NUM_FLAGS + HASH_DIM  # type + bbox + flags + name-hash


def _stable_hash(tok: str) -> int:
    # zlib.crc32 is process-independent, unlike builtin hash() which is salted by
    # PYTHONHASHSEED — we need reproducible features across runs.
    return zlib.crc32(tok.encode("utf-8"))


def hash_tokens(text: str, dim: int = HASH_DIM) -> np.ndarray:
    """Signed hashing-trick bag-of-words, L2-normalised."""
    vec = np.zeros(dim, dtype=np.float32)
    for tok in text.lower().replace("/", " ").split():
        h = _stable_hash(tok)
        vec[h % dim] += 1.0 if (h >> 16) & 1 else -1.0
    norm = float(np.linalg.norm(vec))
    if norm > 0:
        vec /= norm
    return vec


def element_features(obs: Observation) -> tuple[np.ndarray, np.ndarray]:
    """Return ``(elements, mask)`` of shapes ``(MAX_ELEMENTS, ELEM_FEAT)`` and
    ``(MAX_ELEMENTS,)``."""
    feats = np.zeros((MAX_ELEMENTS, ELEM_FEAT), dtype=np.float32)
    mask = np.zeros(MAX_ELEMENTS, dtype=np.int8)
    for i, el in enumerate(obs.elements[:MAX_ELEMENTS]):
        off = 0
        feats[i, off + el.control_type_index()] = 1.0
        off += len(CONTROL_TYPES)
        feats[i, off:off + 4] = el.bbox
        off += 4
        feats[i, off:off + NUM_FLAGS] = (
            float(el.enabled), float(el.focused), float(el.checked), float(el.selected)
        )
        off += NUM_FLAGS
        feats[i, off:off + HASH_DIM] = hash_tokens(el.name)
        mask[i] = 1
    return feats, mask


def featurize(obs: Observation) -> dict:
    elements, mask = element_features(obs)
    return {
        "screenshot": obs.screenshot,
        "elements": elements,
        "element_mask": mask,
        "instruction": hash_tokens(obs.instruction),
    }
