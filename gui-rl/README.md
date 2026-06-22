# gui-rl — reinforcement learning for Windows GUI agents (from scratch)

A small, honest, **runnable** framework for pushing the state of the art of GUI
control on Windows with RL. It is built around three convictions:

1. **The observation is pixels + the accessibility tree.** Vision grounds what a
   thing *is*; the UI Automation tree grounds *where it is and how to invoke it*.
   Actions are **grounded GUI verbs** (click an element/coord, type, key, scroll,
   drag, finish), not raw motor outputs.
2. **The reward is the bottleneck, so instrument the binary.** On a real desktop
   you normally have to *guess* success by scraping pixels — and noisy reward is
   what wrecks RL sample efficiency. Instead we **dynamically instrument apps when
   we launch them** (Detours/MinHook/Frida hooks, ETW, draw-call capture) to read
   ground truth from the process: a *reward/success oracle*, plus recovery of
   controls UIA never exposed (custom-drawn, canvas, Electron, games). This is
   *privileged* information — used for reward and to **repair the observation**,
   never fed to the policy — so the deployed agent still runs on pixels + a11y.
3. **Training is offline-first.** Behavior-clone from logged trajectories, then
   RL-fine-tune (GRPO / PPO). You cannot afford millions of live desktop steps;
   you bootstrap from data you already have and refine with RL. This mirrors what
   actually moves the SOTA for GUI agents.

It follows **Gymnasium + CleanRL** conventions and runs end-to-end on CPU with no
Windows and no GPU, against a simulated desktop — then swaps in a real Windows
backend and a VLM policy behind unchanged interfaces.

## Quickstart

```bash
cd gui-rl
pip install -e .            # numpy + gymnasium
python scripts/smoke.py     # collect -> behavior clone -> GRPO -> evaluate
```

Representative output (reproducible):

```
=== mean success over task suite ===
  light behavior cloning         : 0.33
  BC + GRPO (offline-first)      : 1.00
  GRPO from scratch (same budget): 0.00      # pure RL can't bootstrap from sparse reward

=== instrumentation ablation (final policy) ===
  a11y repaired by instrumentation : 1.00
  raw UIA only (no instrumentation): 0.67     # the dark-mode toggle is invisible to UIA
  lift from instrumentation        : +0.33
```

Two headline results fall out of one run: **offline-first beats pure RL** at equal
budget, and **dynamic instrumentation is worth a measurable success lift** because
it recovers controls the accessibility tree misses.

## CLI

```bash
gui-rl collect --task enable_dark_mode --episodes 200 --out data/expert.jsonl
gui-rl bc      --data data/expert.jsonl --out runs/bc.npz
gui-rl rlft    --algo grpo --init runs/bc.npz --out runs/rlft.npz
gui-rl rlft    --algo grpo --no-repair       # ablate instrumentation
gui-rl eval    --policy runs/rlft.npz
```

## Architecture

```
observation = screenshot + a11y tree  ──►  policy  ──►  grounded action [verb, element, param]
        ▲                                                        │
        │ a11y repair (privileged)                               ▼
   ┌────┴───────────────────────────────────────────────── backend.execute ──┐
   │ Instrument (privileged): launch + hook the binary                         │
   │   • signals  → reward / success oracle                                    │
   │   • events   → in-process event log (window messages, draws, syscalls)    │
   │   • repair() → recover controls UIA never exposed                         │
   └───────────────────────────────────────────────────────────────────────────┘
```

| Layer | Module | Notes |
|---|---|---|
| Action space | `gui_rl/actions.py` | `MultiDiscrete[verb, element, param]`, decoded against the live a11y tree |
| Observation | `gui_rl/features.py` | pixels + featurised a11y; instruction & element names hashed into a shared space for grounding |
| Environment | `gui_rl/env.py` | `WindowsGuiEnv` (Gymnasium); reward = step penalty + milestone shaping + terminal success |
| Backend | `gui_rl/backends/` | `MockGuiBackend` (simulated desktop, runs anywhere) · `WindowsUiaBackend` (real, stub) |
| Instrumentation | `gui_rl/backends/instrument.py` | `Instrument` ABC · `NullInstrument` · `FridaInstrument` (real, stub) |
| Tasks + rewards | `gui_rl/tasks.py` | screen-graph tasks, success/progress oracle, scripted experts for BC data |
| Data | `gui_rl/data.py` | trajectory JSONL (mirrors the node's `trajectory.rs`), dataset, collector |
| Policy | `gui_rl/policies/` | `LinearGuiPolicy` (numpy, factored, CPU) · `VLMGuiPolicy` (torch, stub) |
| Algorithms | `gui_rl/algos/` | `behavior_clone` · `grpo_finetune` · `ppo_finetune` |

The policy **factors** the action: verb/param heads read pooled global features,
while a shared per-element head scores each control from
`[element features ⊕ instruction ⊕ (element-name ⊙ instruction)]`. That product
term is the grounding signal that lets even a linear policy click the control
whose name matches the instruction, over a variable-length a11y tree.

## Going to real Windows

Everything above the backend is OS-agnostic; the same policy trains on the mock
desktop and deploys on Windows. To run for real:

```bash
pip install -e '.[windows]'   # uiautomation, pywinauto, mss, frida
pip install -e '.[torch]'     # VLM policy + GPU training
```

1. **`WindowsUiaBackend`** (`backends/windows_uia.py`) — walk the UIA tree into
   `Element`s, grab the screen with `mss`, map `GuiAction` to UIA Invoke/Toggle or
   `SendInput`.
2. **`FridaInstrument`** (`backends/instrument.py`) — `frida.spawn` the target and
   hook `SendMessageW`/`CreateWindowExW`/registry/draw calls; expose ground-truth
   `signals()` (reward oracle), `drain_events()`, and `repair()` for missing a11y.
3. **`VLMGuiPolicy`** (`policies/vlm.py`) — a vision-language policy over the
   screenshot + tokenised a11y with the same factored heads, so `algos/` train it
   unchanged. Bootstrap from an instruction-tuned VLM, behavior-clone, RL-finetune.

The production node in this repo already harvests UFO2 trajectories in a
compatible shape (`../src/trajectory.rs`), so logged real runs flow straight into
`gui_rl.data` for offline training — closing the loop from deployment to training.

## Tests

```bash
pip install pytest && pytest
```

## License

Apache-2.0, © 2026 UFOAgent, Inc.
