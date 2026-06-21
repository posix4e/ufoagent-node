import asyncio
import faulthandler
import json
import os
import socket
import sys
import threading
import time
import traceback


class Protocol:
    def __init__(self, sock):
        self._sock = sock
        self._out = sock.makefile("w", encoding="utf-8", newline="\n")
        self._lock = threading.Lock()

    def emit(self, payload):
        with self._lock:
            self._out.write(json.dumps(payload, ensure_ascii=False, default=str))
            self._out.write("\n")
            self._out.flush()


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
        self.sock = self.connect_control()
        self.protocol = Protocol(self.sock)
        self.queue = None
        self.active = None
        self.SessionFactory = None
        self.SessionPool = None
        self.clear_config_cache = None
        self.get_ufo_config = None
        self.log_path = os.path.abspath(os.path.join("logs", "ufoagent_worker.log"))
        self.ufo_stdio_path = os.path.abspath(
            os.path.join("logs", "ufoagent_worker_ufo.log")
        )
        self._stdio_file = None
        self._fault_file = None

    def connect_control(self):
        host = os.environ.get("UFOAGENT_WORKER_HOST", "127.0.0.1")
        port = int(os.environ["UFOAGENT_WORKER_PORT"])
        sock = socket.create_connection((host, port), timeout=10)
        sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
        return sock

    def redirect_stdio(self):
        os.makedirs(os.path.dirname(self.ufo_stdio_path), exist_ok=True)
        self._stdio_file = open(self.ufo_stdio_path, "a", encoding="utf-8", buffering=1)
        sys.stdout = self._stdio_file
        sys.stderr = self._stdio_file

    def enable_fault_dumps(self):
        path = os.path.abspath(os.path.join("logs", "ufoagent_worker_faults.log"))
        os.makedirs(os.path.dirname(path), exist_ok=True)
        self._fault_file = open(path, "a", encoding="utf-8", buffering=1)
        faulthandler.enable(file=self._fault_file)
        faulthandler.dump_traceback_later(30, repeat=True, file=self._fault_file)
        self.log("faulthandler_enabled", path=path)

    def log(self, message, **fields):
        try:
            os.makedirs(os.path.dirname(self.log_path), exist_ok=True)
            record = {
                "ts": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
                "message": message,
                **fields,
            }
            with open(self.log_path, "a", encoding="utf-8") as f:
                f.write(json.dumps(record, ensure_ascii=False, default=str))
                f.write("\n")
        except Exception:
            pass

    def emit_diag(self, message, cmd_id=None, **fields):
        self.log(message, cmd_id=cmd_id, **fields)
        payload = {"type": "diag", "message": message, **fields}
        if cmd_id:
            payload["cmd_id"] = cmd_id
        self.protocol.emit(payload)

    def current_cmd_id(self):
        return self.active.cmd_id if self.active else None

    def import_ufo(self):
        self.log("import_ufo_start", cwd=os.getcwd(), argv=sys.argv)
        self.log("import_config_loader_start")
        from config.config_loader import clear_config_cache, get_ufo_config

        self.log("import_config_loader_done")
        self.log("import_session_pool_start")
        from ufo.module.session_pool import SessionFactory, SessionPool

        self.log("import_session_pool_done")
        self.clear_config_cache = clear_config_cache
        self.get_ufo_config = get_ufo_config
        self.SessionFactory = SessionFactory
        self.SessionPool = SessionPool
        self.log("import_ufo_done")

    def refresh_config(self):
        self.log("refresh_config_start")
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
        self.log("refresh_config_done", reloaded=cfg is not None)

    async def run(self):
        self.queue = asyncio.Queue()
        loop = asyncio.get_running_loop()
        self._start_reader(loop)
        self.redirect_stdio()
        self.enable_fault_dumps()
        try:
            self.import_ufo()
            self.refresh_config()
            self.log("worker_ready")
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
        def read_control():
            control_in = self.sock.makefile("r", encoding="utf-8")
            for raw in control_in:
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

        thread = threading.Thread(
            target=read_control, name="ufoagent-control", daemon=True
        )
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
        self.emit_diag("run_received", cmd_id=cmd_id or None, task=task)
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
            session_log_paths = [
                os.path.abspath(getattr(session, "log_path", "")) for session in sessions
            ]
            self.emit_diag(
                "session_created",
                cmd_id=cmd_id,
                task=task,
                cwd=os.getcwd(),
                log_paths=session_log_paths,
                session_count=len(sessions),
            )
        except Exception as exc:
            self.emit_diag(
                "session_create_failed",
                cmd_id=cmd_id,
                error=f"{type(exc).__name__}: {exc}",
            )
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
        self.emit_diag("run_scheduled", cmd_id=cmd_id, task=task)

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
        self.emit_diag("resume_injected", cmd_id=cmd_id, ok=ok)
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
            self.emit_diag("abort_received", cmd_id=cmd_id)
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
            self.emit_diag("ufo_run_start", cmd_id=active.cmd_id, task=active.task)
            await self.SessionPool(active.sessions).run_all()
            status = "halted" if active.halted else "done"
            result = self._summary(active.sessions)
            self.emit_diag("ufo_run_finished", cmd_id=active.cmd_id, status=status)
        except asyncio.CancelledError:
            status = "halted"
            result = "halted by overseer"
            self.emit_diag("ufo_run_cancelled", cmd_id=active.cmd_id)
        except Exception as exc:
            status = "failed"
            result = f"{type(exc).__name__}: {exc}\n{traceback.format_exc(limit=20)}"
            self.emit_diag(
                "ufo_run_failed",
                cmd_id=active.cmd_id,
                error=f"{type(exc).__name__}: {exc}",
            )
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
            self.emit_diag("result_sent", cmd_id=active.cmd_id, status=status)
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
