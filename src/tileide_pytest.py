# SPDX-License-Identifier: GPL-3.0-or-later
"""TileIDE's pytest plugin (loaded with `-p tileide_pytest`): reports what pytest
collects and each result as one JSON line on stdout, after the marker "\x1etileide ".
pytest's own output goes on as usual around them.

{"ev": "collected", "tests": [{"id", "file", "line", "name"}]}
{"ev": "result", "id", "outcome": "passed"|"failed"|"skipped"|"error", "seconds",
 "message", "text", "at": [file, line] | null}
{"ev": "finished", "status"}
"""
import json
import os
import sys

MARK = "\x1etileide "


def emit(**message):
    sys.stdout.write("\n" + MARK + json.dumps(message) + "\n")
    sys.stdout.flush()


def pytest_collection_finish(session):
    tests = []
    for item in session.items:
        file, line, name = item.location
        tests.append({"id": item.nodeid, "file": file, "line": (line or 0) + 1, "name": name})
    emit(ev="collected", tests=tests)


def where(report, root):
    """The deepest traceback line in the project (not in site-packages), else the crash."""
    try:
        entries = report.longrepr.reprtraceback.reprentries
        places = [(e.reprfileloc.path, e.reprfileloc.lineno) for e in entries if getattr(e, "reprfileloc", None)]
        mine = [p for p in places if "site-packages" not in p[0] and (not os.path.isabs(p[0]) or p[0].startswith(root))]
        if mine:
            return list(mine[-1])
        crash = report.longrepr.reprcrash
        return [crash.path, crash.lineno]
    except AttributeError:
        return None


def pytest_runtest_logreport(report):
    # One line per test: its call, or a setup/teardown that failed (or skipped it).
    if report.when != "call" and not (report.failed or report.skipped):
        return
    if report.when == "teardown" and report.skipped:
        return
    outcome = report.outcome
    if report.failed and report.when != "call":
        outcome = "error"
    message, text, at = "", "", None
    if report.failed:
        text = str(report.longrepr)
        crash = getattr(report.longrepr, "reprcrash", None)
        if crash is not None:
            message = crash.message
        else:
            lines = text.strip().splitlines()
            message = lines[-1] if lines else ""
        at = where(report, os.getcwd())
    elif report.skipped and isinstance(report.longrepr, tuple):
        message = str(report.longrepr[2])
    emit(ev="result", id=report.nodeid, outcome=outcome, seconds=round(report.duration, 4), message=message, text=text, at=at)


def pytest_sessionfinish(session, exitstatus):
    emit(ev="finished", status=int(exitstatus))
