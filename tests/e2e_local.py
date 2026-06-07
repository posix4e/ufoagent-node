"""End-to-end local check: drives the real daemon library against a running control plane
(`wrangler dev`). Approves the link via the dev-only endpoint. Run:  python tests/e2e_local.py URL
"""

import json
import os
import sys
import tempfile
import urllib.request

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))

from ufoagentd.controlplane import ControlPlane  # noqa: E402
from ufoagentd.credentials import refresh_once  # noqa: E402
from ufoagentd.linker import link  # noqa: E402
from ufoagentd.ufo_config import agents_yaml_path  # noqa: E402

BASE = sys.argv[1] if len(sys.argv) > 1 else "http://localhost:8788"
VANILLA = "sk-daemon-loop-XYZ789"


def post(path, body):
    req = urllib.request.Request(
        BASE + path, data=json.dumps(body).encode(), headers={"content-type": "application/json"}, method="POST"
    )
    return json.loads(urllib.request.urlopen(req).read())


def main():
    os.environ["UFOAGENT_HOME"] = tempfile.mkdtemp()
    ufo_home = tempfile.mkdtemp()

    post("/dev/ensure-upstream", {"vanilla_key": VANILLA, "model": "gpt-4o-mini"})
    cp = ControlPlane(BASE)

    approved = {}

    def announce(user_code, uri):
        print(f"[announce] user_code={user_code} uri={uri}")
        approved.update(post("/dev/approve", {"user_code": user_code, "agent_name": "daemon-e2e"}))

    agent_id, token = link(cp, "daemon-e2e", "Test/Python", announce, sleep=lambda s: None)
    print(f"[link] agent_id={agent_id} token={'*' * 8}{token[-4:]}")

    cp_auth = ControlPlane(BASE, token=token)
    cred = refresh_once(cp_auth, ufo_home)
    print(f"[cred] lease={cred.lease_id} model={cred.model} key={cred.api_key}")

    yaml_path = agents_yaml_path(ufo_home)
    content = yaml_path.read_text()
    print(f"[agents.yaml] {yaml_path}\n{content}")

    hb = cp_auth.heartbeat("0.1.0", "Test/Python")
    print(f"[heartbeat] {hb}")

    # Assertions
    assert cred.api_key == VANILLA, "vended key mismatch"
    assert f'API_KEY: "{VANILLA}"' in content, "agents.yaml missing API_KEY"
    assert 'API_TYPE: "openai"' in content and "HOST_AGENT:" in content and "APP_AGENT:" in content
    assert 'API_MODEL: "gpt-4o-mini"' in content
    assert hb["ok"] is True
    print("\n✅ E2E PASS: link → approve → credentials → agents.yaml → heartbeat")


if __name__ == "__main__":
    main()
