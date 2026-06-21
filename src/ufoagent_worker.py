import asyncio
import contextlib
import io
import json
import os
import sys
import threading
import traceback


class Protocol:
    def __init__(self):
        self._out = sys.stdout
        self._lock = threading.Lock()

    def emit(self, payload):
        with self._lock:
            self._out.write(json.dumps(payload, ensure_ascii=False, default=str))
            self._out.write("\n")
            self._out.flush()


class CapturedStream(io.TextIOBase):
    def __init__(self, protocol, name, current_cmd_id):
        self.protocol = protocol
        self.name = name
        self.current_cmd_id = current_cmd_id
        self._buffer = ""

    def writable(self):
        return True

    def write(self, data):
        if not data:
            return 0
        text = str(data)
        self._buffer += text
        while "\n" in self._buffer:
            line, self._buffer = self._buffer.split("\n", 1)
            self._emit(line.rstrip("\r"))
        return len(text)

    def flush(self):
        if self._buffer.strip():
            self._emit(self._buffer.strip())
        self._buffer = ""

    def _emit(self, line):
        line = line.strip()
        if not line:
            return
        prefix = "[stderr] " if self.name == "stderr" else ""
        self.protocol.emit(
            {
                "type": "progress",
                "cmd_id": self.current_cmd_id(),
                "message": prefix + line,
            }
        )


class ActiveTask:
    def __init__(self, cmd_id, task, sessions):
        self.cmd_id = cmd_id
        self.task = task
        self.sessions = sessions
        self.session = sessions[0] if sessions else None
        self.future = None
        self.halted = False


class UFOAgentWorker:
    def __init__(self):
        self.protocol = Protocol()
        self.queue = None
        self.active = None
        self.SessionFactory = None
        self.SessionPool = None
        self.clear_config_cache = None
        self.get_ufo_config = None

    def current_cmd_id(self):
        return self.active.cmd_id if self.active else None

    def import_ufo(self):
        from config.config_loader import clear_config_cache, get_ufo_config
        from ufo.module.session_pool import SessionFactory, SessionPool

        self.clear_config_cache = clear_config_cache
        self.get_ufo_config = get_ufo_config
        self.SessionFactory = SessionFactory
        self.SessionPool = SessionPool

    def refresh_config(self):
        cfg = None
        if self.clear_config_cache:
            self.clear_config_cache()
        if self.get_ufo_config:
            cfg = self.get_ufo_config(reload=True)

        # UFO binds `ufo_config = get_ufo_config()` at import time in many modules.
        # Rebind those module globals so a long-lived worker sees credential/config rewrites.
        if cfg is not None:
            for module in list(sys.modules.values()):
                if module is not None and hasattr(module, "ufo_config"):
                    try:
                        setattr(module, "ufo_config", cfg)
                    except Exception:
                        pass
        try:
            import ufo.config as legacy_config

            if hasattr(legacy_config, "Config"):
                legacy_config.Config._instance = None
        except Exception:
            pass

    async def run(self):
        self.queue = asyncio.Queue()
        loop = asyncio.get_running_loop()
        self._start_reader(loop)
        stdout = CapturedStream(self.protocol, "stdout", self.current_cmd_id)
        stderr = CapturedStream(self.protocol, "stderr", self.current_cmd_id)
        with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
            try:
                self.import_ufo()
                self.refresh_config()
                self.protocol.emit({"type": "ready"})
                while True:
                    command = await self.queue.get()
                    await self.handle(command)
            except Exception as exc:
                self.protocol.emit(
                    {
                        "type": "fatal",
                        "error": f"{type(exc).__name__}: {exc}",
                        "traceback": traceback.format_exc(limit=20),
                    }
                )
                raise

    def _start_reader(self, loop):
        def read_stdin():
            for raw in sys.stdin:
                raw = raw.strip()
                if not raw:
                    continue
                try:
                    command = json.loads(raw)
                except Exception as exc:
                    self.protocol.emit(
                        {"type": "error", "error": f"invalid command JSON: {exc}"}
                    )
                    continue
                kind = command.get("type")
                if kind == "resume":
                    self.handle_resume(command)
                elif kind == "abort":
                    self.handle_abort(command, loop)
                elif kind == "ping":
                    self.protocol.emit({"type": "pong"})
                else:
                    loop.call_soon_threadsafe(self.queue.put_nowait, command)

        thread = threading.Thread(target=read_stdin, name="ufoagent-stdin", daemon=True)
        thread.start()

    async def handle(self, command):
        kind = command.get("type")
        if kind == "ping":
            self.protocol.emit({"type": "pong"})
        elif kind == "run":
            await self.handle_run(command)
        elif kind == "resume":
            self.handle_resume(command)
        elif kind == "abort":
            self.handle_abort(command)
        else:
            self.protocol.emit({"type": "error", "error": f"unknown command: {kind}"})

    async def handle_run(self, command):
        cmd_id = str(command.get("cmd_id") or "")
        task = str(command.get("task") or "adhoc")
        request = command.get("request") or task
        if not cmd_id:
            self.protocol.emit({"type": "error", "error": "run missing cmd_id"})
            return
        if self.active and self.active.future and not self.active.future.done():
            self.protocol.emit(
                {
                    "type": "result",
                    "cmd_id": cmd_id,
                    "status": "failed",
                    "result": f"worker is busy with {self.active.cmd_id}",
                }
            )
            return

        try:
            self.refresh_config()
            sessions = self.SessionFactory().create_session(
                task=task,
                mode="normal",
                plan="",
                request=request,
            )
        except Exception as exc:
            self.protocol.emit(
                {
                    "type": "result",
                    "cmd_id": cmd_id,
                    "status": "failed",
                    "result": f"could not create UFO session: {type(exc).__name__}: {exc}",
                }
            )
            return

        self.active = ActiveTask(cmd_id, task, sessions)
        self.active.future = asyncio.create_task(self._run_active(self.active))

    def handle_resume(self, command):
        cmd_id = str(command.get("cmd_id") or "")
        correction = str(command.get("correction") or "").strip()
        if not self.active or self.active.cmd_id != cmd_id:
            self.protocol.emit(
                {
                    "type": "resume_ack",
                    "cmd_id": cmd_id,
                    "ok": False,
                    "error": "no active matching session",
                }
            )
            return
        inject = getattr(self.active.session, "ufoagent_inject_request", None)
        if not callable(inject):
            self.protocol.emit(
                {
                    "type": "resume_ack",
                    "cmd_id": cmd_id,
                    "ok": False,
                    "error": "managed UFO resume adapter is missing",
                }
            )
            return
        if not correction:
            correction = "Overseer requested a correction. Reassess and continue safely."
        ok = bool(inject(f"Overseer correction:\n{correction}"))
        self.protocol.emit({"type": "resume_ack", "cmd_id": cmd_id, "ok": ok})
        self.protocol.emit(
            {
                "type": "progress",
                "cmd_id": cmd_id,
                "message": "Overseer correction queued; UFO will resume in the same session.",
            }
        )

    def handle_abort(self, command, loop=None):
        cmd_id = str(command.get("cmd_id") or "")
        if self.active and self.active.cmd_id == cmd_id:
            self.active.halted = True
            if self.active.future and not self.active.future.done():
                if loop:
                    loop.call_soon_threadsafe(self.active.future.cancel)
                else:
                    self.active.future.cancel()
            self.protocol.emit({"type": "abort_ack", "cmd_id": cmd_id, "ok": True})
        else:
            self.protocol.emit({"type": "abort_ack", "cmd_id": cmd_id, "ok": False})

    async def _run_active(self, active):
        status = "failed"
        result = ""
        try:
            await self.SessionPool(active.sessions).run_all()
            status = "halted" if active.halted else "done"
            result = self._summary(active.sessions)
        except asyncio.CancelledError:
            status = "halted"
            result = "halted by overseer"
        except Exception as exc:
            status = "failed"
            result = f"{type(exc).__name__}: {exc}\n{traceback.format_exc(limit=20)}"
        finally:
            if active.halted and status != "failed":
                status = "halted"
            if not result:
                result = status
            self.protocol.emit(
                {
                    "type": "result",
                    "cmd_id": active.cmd_id,
                    "status": status,
                    "result": result[-8192:],
                }
            )
            if self.active is active:
                self.active = None

    def _summary(self, sessions):
        items = []
        for session in sessions:
            items.append(
                {
                    "task": getattr(session, "task", ""),
                    "log_path": getattr(session, "log_path", ""),
                    "results": getattr(session, "results", []),
                }
            )
        return json.dumps(items, ensure_ascii=False, default=str)


if __name__ == "__main__":
    os.environ.setdefault("PYTHONUTF8", "1")
    asyncio.run(UFOAgentWorker().run())
