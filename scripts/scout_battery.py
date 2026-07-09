#!/usr/bin/env python3
"""Run scout Q1-Q4 via MCP JSON-RPC on stdio, poll, evaluate with full output."""
import json
import subprocess
import sys
import time
import uuid
from pathlib import Path

BIN = Path(__file__).resolve().parents[1] / "target/release/mcp-adjutant"
OUT_DIR = Path("/tmp/scout_battery")

QUERIES = [
    ("Q1", "How does ProjectCacheManager store semantic insights in SQLite?"),
    ("Q2", "Where does the scout cache flow persist and match vector embeddings?"),
    ("Q3", "When should ScoutAgent use ripgrep versus ast_calls?"),
    ("Q4", "How does the scout pick between ripgrep and AST call-site lookup?"),
]

def mcp_call(proc, method, params, req_id):
    msg = {"jsonrpc": "2.0", "id": req_id, "method": method, "params": params}
    proc.stdin.write(json.dumps(msg) + "\n")
    proc.stdin.flush()
    while True:
        line = proc.stdout.readline()
        if not line:
            raise RuntimeError("MCP process closed stdout")
        data = json.loads(line)
        if data.get("id") == req_id:
            if "error" in data:
                raise RuntimeError(data["error"])
            return data.get("result")


def tool_call(proc, name, arguments, req_id):
    return mcp_call(
        proc,
        "tools/call",
        {"name": name, "arguments": arguments},
        req_id,
    )


def poll_job(proc, request_uuid, req_id, timeout=300):
    start = time.time()
    while time.time() - start < timeout:
        res = tool_call(proc, "query_job_status", {"request_uuid": request_uuid}, req_id)
        text = ""
        for block in res.get("content", []):
            if block.get("type") == "text":
                text += block.get("text", "")
        payload = json.loads(text) if text.strip().startswith("{") else {"raw": text}
        if payload.get("terminal"):
            return payload
        time.sleep(2)
    raise TimeoutError(f"job {request_uuid} not terminal after {timeout}s")


def main():
    OUT_DIR.mkdir(exist_ok=True)
    proc = subprocess.Popen(
        [str(BIN)],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        cwd=BIN.parents[1],
    )
    rid = 1
    mcp_call(proc, "initialize", {"protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": {"name": "scout_battery", "version": "0"}}, rid)
    rid += 1
    # notification — no id, server ignores unknown methods with id
    proc.stdin.write(json.dumps({"jsonrpc": "2.0", "method": "notifications/initialized", "params": {}}) + "\n")
    proc.stdin.flush()

    results = {}
    for label, query in QUERIES:
        request_uuid = str(uuid.uuid4())
        print(f"=== {label}: scout_context ===", flush=True)
        t0 = time.time()
        tool_call(proc, "scout_context", {"query": query, "request_uuid": request_uuid}, rid)
        rid += 1
        payload = poll_job(proc, request_uuid, rid)
        rid += 1
        elapsed = time.time() - t0
        output = payload.get("result") or payload.get("raw", json.dumps(payload))
        path = OUT_DIR / f"{label.lower()}.txt"
        path.write_text(output)
        cache_hit = "[CACHE HIT]" in output
        results[label] = {"query": query, "elapsed": round(elapsed, 1), "cache_hit": cache_hit, "path": str(path), "output": output}
        print(f"  {label}: {elapsed:.1f}s cache_hit={cache_hit} len={len(output)}", flush=True)

    eval_scores = {}
    for label, query in QUERIES:
        request_uuid = str(uuid.uuid4())
        output = results[label]["output"]
        print(f"=== {label}: evaluate ===", flush=True)
        tool_call(
            proc,
            "evaluate_agent_performance",
            {
                "target_agent": "Phase_1_Scout",
                "original_task": query,
                "received_output": output,
                "request_uuid": request_uuid,
            },
            rid,
        )
        rid += 1
        payload = poll_job(proc, request_uuid, rid, timeout=120)
        rid += 1
        eval_text = payload.get("result") or json.dumps(payload)
        (OUT_DIR / f"{label.lower()}_eval.txt").write_text(eval_text)
        eval_scores[label] = eval_text
        print(f"  {label} eval: {eval_text[:200]}...", flush=True)

    summary = {
        "results": {k: {kk: vv for kk, vv in v.items() if kk != "output"} for k, v in results.items()},
        "evaluations": eval_scores,
    }
    (OUT_DIR / "summary.json").write_text(json.dumps(summary, indent=2))
    print(json.dumps(summary, indent=2))
    proc.terminate()
    return 0


if __name__ == "__main__":
    sys.exit(main())
