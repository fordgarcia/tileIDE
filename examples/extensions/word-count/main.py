#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""A minimal TileIDE extension: JSON-RPC 2.0 over stdin/stdout, Content-Length framing
(the same framing as LSP). See docs/extensions.md."""
import json
import sys


def read():
    length = None
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        line = line.strip()
        if not line:
            break
        name, _, value = line.decode().partition(":")
        if name.lower() == "content-length":
            length = int(value)
    return json.loads(sys.stdin.buffer.read(length))


def send(message):
    body = json.dumps(dict(message, jsonrpc="2.0")).encode()
    sys.stdout.buffer.write(b"Content-Length: %d\r\n\r\n%s" % (len(body), body))
    sys.stdout.buffer.flush()


def summary(text):
    return f"{len(text.split())} words, {text.count(chr(10)) + (1 if text and not text.endswith(chr(10)) else 0)} lines, {len(text)} characters"


while (message := read()) is not None:
    method, params = message.get("method"), message.get("params") or {}
    if method == "command/run":
        text = params.get("text")
        if text is None:
            send({"method": "window/notice", "params": {"message": "Word count: no file is focused."}})
        elif params["command"] == "word-count.count":
            name = (params.get("file") or "untitled").rsplit("/", 1)[-1]
            send({"method": "window/notice", "params": {"message": f"{name}: {summary(text)}"}})
        else:
            send({"method": "editor/insert", "params": {"text": summary(text)}})
    elif method == "exit":
        break
    if "id" in message:
        send({"id": message["id"], "result": None})
