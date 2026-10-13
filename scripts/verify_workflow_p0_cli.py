"""Opt-in real-provider acceptance against an isolated, already-running backend.

This starts actual installed agents using their existing user authentication.
Pass a new temporary workspace and the debug asset directory explicitly. It
does not change credentials, start a server, reset storage or delete artifacts.
"""

import argparse
import base64
import ctypes
import hashlib
import json
import os
from pathlib import Path
import sqlite3
import sys
import time
from urllib.error import HTTPError
from urllib.request import Request, urlopen
import uuid


def call(base, route, payload=None, method=None):
    body = None if payload is None else json.dumps(payload).encode("utf-8")
    request = Request(
        base.rstrip("/") + route,
        data=body,
        method=method or ("GET" if body is None else "POST"),
        headers={"Content-Type": "application/json"},
    )
    try:
        with urlopen(request, timeout=40) as response:
            result = json.load(response)
    except HTTPError as error:
        raise RuntimeError(f"{route}: HTTP {error.code}: {error.read().decode()}") from error
    if result.get("success") is False:
        raise RuntimeError(f"{route}: {result.get('message') or result}")
    return result.get("data", result)


def config(provider, claude_model=None):
    selected = {"executor": provider, "variant": "DEFAULT", "permission_policy": "AUTO"}
    if provider == "CLAUDE_CODE" and claude_model:
        selected["model_id"] = claude_model
    return selected


def graph(split, claude_model=None, stopping=False):
    node_specs = (
        [
            ("split", "task_splitter", "CLAUDE_CODE",
             "Read materials.txt and the accepted request file. Create exactly three internal "
             "tasks: alpha and beta with no dependencies, gamma depending on alpha and beta. "
             "Alpha reads materials.txt and writes artifacts/alpha.txt with the source token "
             "and the sum of the values. Beta reads materials.txt and writes artifacts/beta.txt "
             "with the source token and the count of values. Gamma reads the accepted alpha/beta "
             "deliveries and writes artifacts/gamma.txt containing their source token, sum and count. "
             "Each instruction must require the executor to publish its actual runtime handoff. "
             "Use the exact manifest identity and path from your runtime envelope. "
             "Publish complete instruction files before atomically publishing tasks.json."),
            ("worker-a", "agent", "CODEX", "Read the assigned instruction and its referenced files; execute it exactly."),
            ("worker-b", "agent", "CLAUDE_CODE", "Read the assigned instruction and its referenced files; execute it exactly."),
            ("summary", "agent", "CLAUDE_CODE", "Read all accepted subtask deliveries. Write artifacts/final.txt containing the actual source token, sum, count and a concise summary. Publish your handoff."),
        ] if split else [
            ("analysis", "agent", "CODEX", "Read materials.txt using file tools. Write artifacts/analysis.txt with source_token=<the actual token> and sum=<sum of the values>. Publish the required runtime handoff with artifacts/analysis.txt as an artifact. Do not ask questions."),
            ("review", "agent", "CLAUDE_CODE", "Read the accepted upstream handoff and artifacts/analysis.txt using file tools. Write artifacts/final.txt with the exact source token and sum from that file plus reviewed=yes. Publish the required runtime handoff with artifacts/final.txt as an artifact. Do not ask questions."),
        ]
    )
    nodes = [{"id": "start", "type": "start", "data": {}}]
    nodes.extend(
        {"id": name, "type": kind,
         "data": {"prompt_template": prompt, "executor_config": config(provider, claude_model)}}
        for name, kind, provider, prompt in node_specs
    )
    nodes.append({"id": "end", "type": "end", "data": {}})
    pairs = (
        [("start", "split"), ("split", "worker-a"), ("split", "worker-b"),
         ("worker-a", "summary"), ("worker-b", "summary"), ("summary", "end")]
        if split else [("start", "analysis"), ("analysis", "review"), ("review", "end")]
    )
    if stopping:
        nodes = [nodes[0], {
            "id": "wait", "type": "agent", "data": {
                "executor_config": config("CODEX"),
                "prompt_template": "Run a native command that writes stop-test-started.txt "
                "containing the PID of that command process, then sleeps for 300 seconds "
                "in the same command. Keep the wait in the foreground. This pause is "
                "deliberate acceptance work; execute the command now. Only after the wait "
                "finishes, write artifacts/final.txt and publish your runtime handoff.",
            },
        }, nodes[-1]]
        pairs = [("start", "wait"), ("wait", "end")]
    return {"version": 2, "nodes": nodes,
            "edges": [{"id": f"{a}-{b}", "source": a, "target": b, "type": "default"} for a, b in pairs]}


def process_is_running(pid):
    if os.name != "nt":
        try:
            os.kill(pid, 0)
        except ProcessLookupError:
            return False
        return True
    kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    kernel.OpenProcess.argtypes = [ctypes.c_uint32, ctypes.c_int, ctypes.c_uint32]
    kernel.OpenProcess.restype = ctypes.c_void_p
    kernel.GetExitCodeProcess.argtypes = [ctypes.c_void_p, ctypes.POINTER(ctypes.c_uint32)]
    kernel.CloseHandle.argtypes = [ctypes.c_void_p]
    handle = kernel.OpenProcess(0x1000, False, pid)
    if not handle:
        error = ctypes.get_last_error()
        if error == 87:  # ERROR_INVALID_PARAMETER: PID no longer exists.
            return False
        raise ctypes.WinError(error)
    try:
        exit_code = ctypes.c_uint32()
        if not kernel.GetExitCodeProcess(handle, ctypes.byref(exit_code)):
            raise ctypes.WinError(ctypes.get_last_error())
        return exit_code.value == 259  # STILL_ACTIVE
    finally:
        kernel.CloseHandle(handle)


def verify_audits(args, db_path, agent_ids):
    """Observe settled audit indexes, verify actual bytes and OS process exit."""
    deadline = time.monotonic() + 60
    while True:
        audits = []
        with sqlite3.connect(f"file:{db_path.as_posix()}?mode=ro", uri=True) as db:
            db.row_factory = sqlite3.Row
            for agent_id in sorted(agent_ids):
                rows = db.execute(
                    "SELECT s.agent_run_id,s.run_attempt_id,s.integrity_status,"
                    "s.manifest_relative_path,s.final_checksum,s.closed_at,"
                    "p.registry_status,p.pid,p.host_pid FROM native_audit_streams s "
                    "LEFT JOIN agent_process_registry p ON p.run_attempt_id=s.run_attempt_id "
                    "WHERE s.agent_run_id=?", (agent_id,),
                ).fetchall()
                audits.extend(dict(row) for row in rows)
        assert not any(a["integrity_status"] == "audit_failed" for a in audits), \
            "An actual provider audit failed after workflow completion"
        if ({a["agent_run_id"] for a in audits} == agent_ids
                and all(a["integrity_status"] == "complete" and a["closed_at"]
                        and a["registry_status"] == "exited" for a in audits)):
            break
        if time.monotonic() >= deadline:
            raise RuntimeError("Provider audits or owned processes did not settle within 60s")
        time.sleep(1)
    for audit in audits:
        manifest_path = args.assets.resolve() / audit["manifest_relative_path"]
        manifest_name = str(manifest_path)
        if os.name == "nt":
            manifest_name = "\\\\?\\" + manifest_name
        with open(manifest_name, "r", encoding="utf-8") as source:
            manifest = json.load(source)
        assert manifest["integrity_status"] == "complete" and manifest["closed_at"]
        assert uuid.UUID(manifest["run_attempt_id"]).bytes == audit["run_attempt_id"]
        frames_name = str(args.assets.resolve() / manifest["frames_relative_path"])
        if os.name == "nt":
            frames_name = "\\\\?\\" + frames_name
        with open(frames_name, "rb") as source:
            content = source.read()
        assert hashlib.sha256(content).hexdigest() == manifest["final_checksum"] == audit["final_checksum"]
        lines = content.splitlines()
        assert len(lines) == manifest["frame_count"] > 0
        for sequence, line in enumerate(lines, 1):
            frame = json.loads(line)
            assert frame["sequence"] == sequence
            payload = base64.b64decode(frame["payload_base64"], validate=True)
            assert hashlib.sha256(payload).hexdigest() == frame["payload_checksum"]
        checked_pids = [pid for pid in (audit["pid"], audit["host_pid"]) if pid is not None]
        assert all(not process_is_running(pid) for pid in checked_pids), \
            "Audit settlement left an actual provider or host running"
        audit.update({
            "agent_run_id": str(uuid.UUID(bytes=audit["agent_run_id"])),
            "run_attempt_id": str(uuid.UUID(bytes=audit["run_attempt_id"])),
            "provider_id": manifest["provider_id"],
            "frame_count": manifest["frame_count"], "os_checked_pids": checked_pids,
        })
    return audits


def verify_stop(args, workspace, db_path, attempt, run_id):
    marker = workspace / "stop-test-started.txt"
    deadline = time.monotonic() + min(args.timeout, 180)
    while not marker.is_file() and time.monotonic() < deadline:
        result = call(args.base, f"/api/local/v1/workflow-runs/{run_id}")
        if result["status"] in ("succeeded", "failed", "canceled"):
            raise RuntimeError(f"Stop test ended before its native command started: {result}")
        time.sleep(2)
    assert marker.is_file(), "The real native wait command never started"
    command_pid = int(marker.read_text(encoding="utf-8-sig").strip())
    before = call(args.base, f"/api/local/v1/workflow-runs/{run_id}")
    assert before["status"] == "running", "Stop test must interrupt a running workflow"
    endpoint = f"/api/workflow-management/instances/{attempt['id']}/stop"
    request = {"run_id": run_id, "request_id": "stop-native-cli"}
    first = call(args.base, endpoint, request)
    deadline = time.monotonic() + min(args.timeout, 180)
    while True:
        with sqlite3.connect(f"file:{db_path.as_posix()}?mode=ro", uri=True) as db:
            processes = db.execute(
                "SELECT p.registry_status,p.pid,p.host_pid FROM agent_process_registry p "
                "JOIN agent_run_attempts a ON a.id=p.run_attempt_id "
                "JOIN node_executions n ON n.agent_run_id=a.agent_run_id WHERE n.run_id=?",
                (uuid.UUID(run_id).bytes,),
            ).fetchall()
            slots, = db.execute("SELECT count(*) FROM workflow_project_slots WHERE run_id=?",
                                (uuid.UUID(run_id).bytes,)).fetchone()
            intents = db.execute("SELECT state FROM workflow_stop_intents WHERE run_id=?",
                                 (uuid.UUID(run_id).bytes,)).fetchall()
            commands = db.execute(
                "SELECT delivery_status,delivery_attempts,last_error FROM orchestration_outbox "
                "WHERE orchestration_run_id=?",
                (uuid.UUID(run_id).bytes,),
            ).fetchall()
        # Send stop once, then observe the background retry. Repeated POSTs in
        # this wait would hide a dispatcher that only progresses on user input.
        if (processes and all(p[0] == "exited" for p in processes) and slots == 0
                and intents == [("delivered",)] and commands
                and all(c[0] == "delivered" for c in commands)):
            break
        if time.monotonic() >= deadline:
            raise RuntimeError(f"Background stop did not settle: processes={processes}, "
                               f"slots={slots}, intents={intents}, commands={commands}")
        time.sleep(2)
    replay = call(args.base, endpoint, request)
    assert replay["stop_status"] == "confirmed"
    assert first["affected_run_ids"] == replay["affected_run_ids"]
    result = call(args.base, f"/api/local/v1/workflow-runs/{run_id}")
    assert result["status"] == "canceled"
    assert next(n for n in result["nodes"] if n["node_id"] == "end")["status"] != "succeeded"
    assert not (workspace / "artifacts/final.txt").exists()
    checked_pids = {command_pid}
    checked_pids.update(pid for p in processes for pid in p[1:] if pid is not None)
    assert all(not process_is_running(pid) for pid in checked_pids), \
        "Confirmed stop left an actual provider, host or native command running"
    audits = verify_audits(args, db_path, {
        uuid.UUID(node["agent_run_id"]).bytes for node in result["nodes"]
        if node.get("agent_run_id")
    })
    evidence_path = workspace / "acceptance-evidence.json"
    evidence_path.write_text(json.dumps({
        "scenario": "stop", "before": before, "first_stop": first,
        "replayed_stop": replay, "result": result, "processes": processes,
        "native_command_pid": command_pid, "os_checked_pids": sorted(checked_pids),
        "stop_intents": intents, "outbox_commands": commands, "audits": audits,
    }, indent=2, ensure_ascii=False), encoding="utf-8")
    print(f"Verified live native stop, replay and process settlement. Evidence: {evidence_path}", flush=True)


def run(args):
    workspace = args.workspace.resolve()
    if workspace.exists():
        raise RuntimeError("Use a new, nonexistent workspace to avoid touching existing projects")
    db_path = args.assets.resolve() / "db.v2.sqlite"
    if not db_path.is_file():
        raise RuntimeError("The explicit debug asset directory has no running backend database")
    workspace.mkdir(parents=True)
    token = uuid.uuid4().hex
    (workspace / "materials.txt").write_text(
        f"source_token={token}\nvalues=17,23,31\n", encoding="utf-8"
    )
    split = args.scenario == "split"
    name = f"P0 real CLI {args.scenario} {uuid.uuid4().hex[:8]}"
    project = call(args.base, "/api/local/v1/projects/open", {
        "name": name, "color": "210 80% 52%", "directory_path": str(workspace),
    })
    project_id = project["id"]
    statuses = call(args.base, f"/api/local/v1/fallback/project_statuses?project_id={project_id}")
    statuses = statuses.get("project_statuses", statuses)
    status = next(s for s in statuses if s["name"].lower() == "todo")
    task = call(args.base, "/api/local/v1/tasks", {
        "project_id": project_id, "status_id": status["id"], "title": name,
        "description": "Real CLI file-driven acceptance", "sort_order": 0,
        "extension_metadata": {},
    })
    project_file = workspace / ".vibe-kanban/project.json"
    original_tasks = {t["id"] for t in json.loads(project_file.read_text(encoding="utf-8"))["tasks"]}
    attempt = call(args.base, f"/api/local/v1/projects/{project_id}/tasks/{task['id']}/workflow-attempts", {
        "name": name, "directory_path": str(workspace), "graph_json": json.dumps(graph(split, args.claude_model, args.scenario == "stop")),
    })
    workflow_id = attempt["workflow_id"]
    publication = call(args.base, f"/api/local/v1/workflows/{workflow_id}")
    call(args.base, f"/api/local/v1/workflows/{workflow_id}", {
        "expected_revision": publication["revision"], "main_agent_config": config("CODEX"),
        "main_agent_prompt": "Manage the frozen workflow through its scoped MCP tools. For an accepted split, read the actual task/instruction files, write and submit a valid dispatch plan. Put alpha and beta in stage 0 on worker-a and worker-b respectively, gamma in stage 1 on worker-a. Do not execute the worker tasks yourself. Use runtime-provided schema and identities. Do not start another Run or business Task. Finish after workflow_dispatch is accepted.",
    }, method="PUT")
    prepared = call(args.base, "/api/workflow-management/prepare-main-session", {
        "project_id": project_id, "workflow_id": workflow_id,
        "issue_id": task["id"], "request_id": "prepare-cli",
    })
    accepted = call(args.base, f"/api/workflow-management/instances/{attempt['id']}/submit", {
        "request_id": "start-cli", "action": "start",
        "input_text": "Read materials.txt in this project. Produce the requested report using actual file handoffs. Work only in this temporary project. Do not make commits or network changes.",
        "material_paths": ["materials.txt"], "scope": {"type": "all"},
    })
    run_id = accepted["run_id"]
    print(f"Accepted {args.scenario}: project={project_id} run={run_id}", flush=True)
    evidence_path = workspace / "acceptance-evidence.json"
    deadline = time.monotonic() + args.timeout
    previous = None
    result = None
    try:
        if args.scenario == "stop":
            verify_stop(args, workspace, db_path, attempt, run_id)
            return
        while time.monotonic() < deadline:
            result = call(args.base, f"/api/local/v1/workflow-runs/{run_id}")
            state = (result["status"], tuple((n["node_id"], n["iteration"], n["status"]) for n in result["nodes"]),
                     tuple(b["state"] for b in result.get("split_batches", [])))
            if state != previous:
                print(state, flush=True)
                previous = state
            if result["status"] in ("succeeded", "failed", "canceled"):
                break
            time.sleep(3)
        if result is None or result["status"] != "succeeded":
            errors = [(n["node_id"], n.get("error_text")) for n in result.get("nodes", [])] if result else []
            raise RuntimeError(f"Run did not succeed: {result and result['status']}; {result and result.get('error_text')}; {errors}")
        delivered = (workspace / "artifacts/final.txt").read_text(encoding="utf-8")
        assert token in delivered and "71" in delivered, "Final delivery did not consume the unique actual material"
        preview = call(args.base, f"/api/workflow-runs/{run_id}/files/content?path=artifacts%2Ffinal.txt")
        assert preview["content"] == delivered, "Workflow file preview did not serve the actual delivery"
        with urlopen(args.base.rstrip("/") + preview["raw_url"], timeout=40) as response:
            assert response.read() == (workspace / "artifacts/final.txt").read_bytes()
        current_tasks = {t["id"] for t in json.loads(project_file.read_text(encoding="utf-8"))["tasks"]}
        assert current_tasks == original_tasks, "Workflow subtasks leaked into project business Tasks"
        if not split:
            assert "reviewed=yes" in delivered
            review = next(n for n in result["nodes"] if n["node_id"] == "review")
            assert token not in review["input_text"], "Downstream received copied result instead of reading files"
        for handoff in result["handoffs"]:
            for reference in handoff["files"]:
                content = (workspace / reference["path"]).read_bytes()
                assert hashlib.sha256(content).hexdigest() == reference["sha256"]
        if split:
            batch = result["split_batches"][0]
            assert all(s["state"] == "succeeded" for s in batch["subtasks"])
            assert len({s["session_id"] for s in batch["subtasks"]}) == 3
            nodes = {n["id"]: n for n in result["nodes"]}
            alpha, beta, gamma = (next(s for s in batch["subtasks"] if s["subtask_id"] == name) for name in ("alpha", "beta", "gamma"))
            a, b, g = (nodes[s["node_execution_id"]] for s in (alpha, beta, gamma))
            assert max(a["started_at"], b["started_at"]) < min(a["finished_at"], b["finished_at"]), "Parallel executions did not overlap"
            assert g["started_at"] >= max(a["finished_at"], b["finished_at"])
            assert len([n for n in result["nodes"] if n["node_id"] == "summary"]) == 1
            assert batch["handover"]["session_id"] == prepared["session"]["id"]
        with sqlite3.connect(f"file:{db_path.as_posix()}?mode=ro", uri=True) as db:
            agent_ids = {uuid.UUID(n["agent_run_id"]).bytes for n in result["nodes"] if n.get("agent_run_id")}
            if split:
                handover = batch["handover"]
                assert handover["agent_run_id"], "Automatic dispatch had no real main-Agent Run"
                main_agent_id = uuid.UUID(handover["agent_run_id"]).bytes
                role, = db.execute(
                    "SELECT json_extract(t.input_message,'$.role') FROM agent_turns t WHERE t.agent_run_id=?",
                    (main_agent_id,),
                ).fetchone()
                assert role == "system", "Automatic dispatch must be a System turn in the original Session"
                agent_ids.add(main_agent_id)
            traces = []
            for agent_id in agent_ids:
                rows = db.execute("SELECT event_envelope FROM agent_events WHERE agent_run_id=? ORDER BY run_attempt_number,sequence", (agent_id,)).fetchall()
                traces.extend(json.loads(row[0]) for row in rows)
        audits = verify_audits(args, db_path, agent_ids)
        evidence_path.write_text(json.dumps({"scenario": args.scenario, "result": result,
                                           "events": traces, "audits": audits},
                                          indent=2, ensure_ascii=False), encoding="utf-8")
        print(f"Verified actual files, identities, execution times and complete audits. Evidence: {evidence_path}", flush=True)
    except BaseException:
        if result is not None:
            evidence_path.write_text(json.dumps({"scenario": args.scenario, "result": result}, indent=2, ensure_ascii=False), encoding="utf-8")
        try:
            call(args.base, f"/api/workflow-management/instances/{attempt['id']}/stop", {"run_id": run_id, "request_id": "stop-cli"})
        except Exception as stop_error:
            print(f"Could not stop the test Run: {stop_error}", file=sys.stderr, flush=True)
        raise


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--base", required=True)
    parser.add_argument("--assets", required=True, type=Path)
    parser.add_argument("--workspace", required=True, type=Path)
    parser.add_argument("--scenario", choices=("ordinary", "split", "stop"), required=True)
    parser.add_argument("--timeout", type=int, default=900)
    parser.add_argument("--claude-model", help="Optional per-test model override; native user settings remain unchanged")
    run(parser.parse_args())
