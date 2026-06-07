"""Offline unit tests (no control plane, no pytest required).
Run:  python tests/test_unit.py   (or `pytest`)."""

import os
import sys
import tempfile

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))

from ufoagentd.controlplane import Credential  # noqa: E402
from ufoagentd.ufo_config import render_agents_yaml  # noqa: E402
from ufoagentd.updater import needs_update, version_tuple  # noqa: E402


def test_render_agents_yaml():
    cred = Credential("openai", "https://api.openai.com/v1", "gpt-4o", "sk-secret", 999, "lse_x")
    txt = render_agents_yaml(cred)
    for section in ("HOST_AGENT:", "APP_AGENT:"):
        assert section in txt
    assert 'API_TYPE: "openai"' in txt
    assert 'API_KEY: "sk-secret"' in txt
    assert 'API_MODEL: "gpt-4o"' in txt
    assert "VISUAL_MODE: true" in txt  # bool not quoted


def test_yaml_escaping():
    cred = Credential("openai", "https://x/v1", "m", 'a"b\\c', 1, "l")
    txt = render_agents_yaml(cred)
    assert 'API_KEY: "a\\"b\\\\c"' in txt


def test_version_tuple_and_needs_update():
    assert version_tuple("1.2.3") == (1, 2, 3)
    assert version_tuple("0.1.0-rc1") == (0, 1, 0)
    assert needs_update("0.1.0", "0.2.0") is True
    assert needs_update("0.2.0", "0.2.0") is False
    assert needs_update("1.0.0", "0.9.9") is False


def test_store_roundtrip():
    with tempfile.TemporaryDirectory() as d:
        os.environ["UFOAGENT_HOME"] = d
        # Re-import store fresh so config_dir() picks up the env.
        import importlib

        from ufoagentd import store

        importlib.reload(store)
        store.update_config(control_plane="https://ufoagent.xyz", ufo_home="C:/UFO")
        cfg = store.load_config()
        assert cfg["control_plane"] == "https://ufoagent.xyz"
        assert cfg["ufo_home"] == "C:/UFO"
        store.set_token("tok-123")
        assert store.get_token() == "tok-123"
        store.clear_token()
        assert store.get_token() is None


def test_credential_from_json():
    c = Credential.from_json(
        {"provider": "openai", "base_url": "b", "model": "m", "api_key": "k", "expires_at": "42", "lease_id": "l"}
    )
    assert c.expires_at == 42 and c.api_key == "k"


def _run():
    tests = [v for k, v in sorted(globals().items()) if k.startswith("test_") and callable(v)]
    failed = 0
    for t in tests:
        try:
            t()
            print(f"  ✓ {t.__name__}")
        except AssertionError as e:
            failed += 1
            print(f"  ✗ {t.__name__}: {e}")
    print(f"\n{len(tests) - failed}/{len(tests)} passed")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(_run())
