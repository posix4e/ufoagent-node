# Self-Hosted E2E

`run-e2e.sh` defaults to the `provisioned` VM snapshot. That snapshot already has UFO2 source,
Python, the virtualenv, and heavy requirements installed. Each run still installs the freshly built
UFOAgent node and runs bootstrap, which reapplies the managed UFO config and verifies the MCP registry
before skipping the dependency install.

Use the full cold path occasionally:

```bash
SNAP=cold bash tests/selfhost/run-e2e.sh
```

Recreate the fast snapshot after changing the base VM or UFO provisioning:

```bash
CREATE_PROVISIONED_SNAPSHOT=1 \
UFOAGENT_INSTALLER_PATH=/path/to/ufoagent-setup.exe \
bash tests/selfhost/run-e2e.sh
```

That reverts `cold`, installs/provisions UFO2 once, stops after the install phase passes, shuts the VM
down, and replaces the `provisioned` snapshot.
