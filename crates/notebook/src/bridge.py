# SPDX-License-Identifier: GPL-3.0-or-later
"""TileIDE's kernel bridge: runs one Jupyter kernel with jupyter_client and speaks
JSON lines with TileIDE on stdin/stdout, so TileIDE needs no ZeroMQ.

argv: kernel name, working directory.
in:  {"op": "execute", "id": N, "code": "..."} | {"op": "complete", "id": N, "code": "...", "cursor": C}
     {"op": "interrupt"} | {"op": "restart"} | {"op": "shutdown"}
out: {"ev": "ready", "language": ...} | {"ev": "status", "state": ...} | {"ev": "count", "id", "count"}
     {"ev": "output", "id", "output": <nbformat output>} | {"ev": "clear", "id"} | {"ev": "done", "id"}
     {"ev": "complete", "id", "matches": [...], "start", "end"} (cursor offsets in characters)
     {"ev": "restarted"} | {"ev": "fatal", "message": ...}
"""
import json
import queue
import signal
import sys
import threading


def out(**message):
    sys.stdout.write(json.dumps(message) + "\n")
    sys.stdout.flush()


try:
    from jupyter_client import KernelManager
except ImportError as e:
    out(ev="fatal", message=f"jupyter_client is not installed for {sys.executable} ({e})")
    sys.exit(1)

kernel_name = sys.argv[1] if len(sys.argv) > 1 else "python3"
cwd = sys.argv[2] if len(sys.argv) > 2 else None
try:
    km = KernelManager(kernel_name=kernel_name)
    km.start_kernel(cwd=cwd)
    kc = km.client()
    kc.start_channels()
    kc.wait_for_ready(timeout=60)
except Exception as e:
    out(ev="fatal", message=f"cannot start the {kernel_name} kernel: {e}")
    sys.exit(1)

def stop(*_):
    # TileIDE ended (or was killed): take the kernel with us.
    km.shutdown_kernel(now=True)
    sys.exit(0)


signal.signal(signal.SIGTERM, stop)
spec = km.kernel_spec
out(ev="ready", language=getattr(spec, "language", ""), display=getattr(spec, "display_name", kernel_name))

commands = queue.Queue()


def read_commands():
    for line in sys.stdin:
        try:
            commands.put(json.loads(line))
        except ValueError:
            pass
    commands.put({"op": "shutdown"})


threading.Thread(target=read_commands, daemon=True).start()
pending = {}
# Completion requests in flight: shell msg_id -> TileIDE's request id.
completing = {}


def output(kind, content):
    if kind == "stream":
        return {"output_type": "stream", "name": content["name"], "text": content["text"]}
    if kind == "error":
        return {"output_type": "error", "ename": content["ename"], "evalue": content["evalue"], "traceback": content["traceback"]}
    result = {"output_type": kind, "data": content.get("data", {}), "metadata": content.get("metadata", {})}
    if kind == "execute_result":
        result["execution_count"] = content.get("execution_count")
    return result


while True:
    try:
        while True:
            command = commands.get_nowait()
            op = command.get("op")
            if op == "execute":
                pending[kc.execute(command["code"], store_history=True)] = command["id"]
            elif op == "complete":
                completing[kc.complete(command["code"], command["cursor"])] = command["id"]
            elif op == "interrupt":
                km.interrupt_kernel()
            elif op == "restart":
                km.restart_kernel()
                pending.clear()
                out(ev="restarted")
            elif op == "shutdown":
                kc.stop_channels()
                km.shutdown_kernel(now=True)
                sys.exit(0)
    except queue.Empty:
        pass
    # Shell replies: completions are answered; the rest is on iopub already.
    try:
        while True:
            reply = kc.get_shell_msg(timeout=0)
            request = completing.pop(reply["parent_header"].get("msg_id"), None)
            if request is not None:
                c = reply["content"]
                out(ev="complete", id=request, matches=c.get("matches", []), start=c.get("cursor_start", 0), end=c.get("cursor_end", 0))
    except queue.Empty:
        pass
    try:
        message = kc.get_iopub_msg(timeout=0.05)
    except queue.Empty:
        if not km.is_alive():
            out(ev="fatal", message="the kernel died")
            sys.exit(1)
        continue
    kind, content = message["msg_type"], message["content"]
    cell = pending.get(message["parent_header"].get("msg_id"))
    if kind == "status":
        out(ev="status", state=content["execution_state"])
        if content["execution_state"] == "idle" and cell is not None:
            pending.pop(message["parent_header"]["msg_id"], None)
            out(ev="done", id=cell)
    elif cell is None:
        continue
    elif kind == "execute_input":
        out(ev="count", id=cell, count=content.get("execution_count"))
    elif kind in ("stream", "display_data", "execute_result", "error"):
        out(ev="output", id=cell, output=output(kind, content))
    elif kind == "clear_output":
        out(ev="clear", id=cell)
