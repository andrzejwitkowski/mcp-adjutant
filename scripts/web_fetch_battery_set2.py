#!/usr/bin/env python3
"""Second web_fetch battery — different query set."""
import json
import subprocess
import sys
import time
import uuid
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
BIN = REPO_ROOT / "target/release/mcp-adjutant"
OUT_DIR = Path("/tmp/web_fetch_battery_set2")

QUERIES = [
    ("Q1_k8s", "Kubernetes liveness vs readiness probes: when to use each"),
    ("Q2_wasm", "WebAssembly vs native plugins for extensible server architectures"),
    ("Q3_graphql", "GraphQL federation tradeoffs compared to REST for internal microservices"),
    ("Q4_rust_concur", "Rust patterns for shared mutable state: Arc, Mutex, RwLock, channels"),
    ("Q5_rag", "RAG context window management: chunking, reranking, and token budgets"),
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
    return mcp_call(proc, "tools/call", {"name": name, "arguments": arguments}, req_id)


def poll_job(proc, request_uuid, req_id, timeout=420):
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
        time.sleep(3)
    raise TimeoutError(f"job {request_uuid} not terminal after {timeout}s")


def evaluate_output(output: str, phrase: str, status: str) -> dict:
    text = str(output)
    failed = status == "failed"
    cache_hit = "[CACHE HIT]" in text
    has_report = len(text.strip()) > 200 and not failed
    has_sources = "http" in text.lower() or "source" in text.lower() or cache_hit
    score = 0
    notes = []
    if failed:
        notes.append("job failed")
    elif cache_hit:
        score += 3
        notes.append("cache hit")
    if has_report:
        score += 4
        notes.append("substantive report")
    if has_sources:
        score += 3
        notes.append("references sources")
    return {
        "score": min(score, 10),
        "cache_hit": cache_hit,
        "failed": failed,
        "chars": len(text),
        "notes": notes,
        "phrase": phrase,
    }


def main():
    OUT_DIR.mkdir(exist_ok=True)
    proc = subprocess.Popen(
        [str(BIN)],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        cwd=REPO_ROOT,
    )
    rid = 1
    mcp_call(
        proc,
        "initialize",
        {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {"name": "web_fetch_battery_set2", "version": "0"},
        },
        rid,
    )
    rid += 1
    proc.stdin.write(
        json.dumps({"jsonrpc": "2.0", "method": "notifications/initialized", "params": {}})
        + "\n"
    )
    proc.stdin.flush()

    results = {}
    for label, phrase in QUERIES:
        request_uuid = str(uuid.uuid4())
        print(f"=== {label}: web_fetch ===", flush=True)
        t0 = time.time()
        tool_call(
            proc,
            "web_fetch",
            {
                "search_phrase": phrase,
                "request_uuid": request_uuid,
                "force_refresh": True,
            },
            rid,
        )
        rid += 1
        payload = poll_job(proc, request_uuid, rid)
        rid += 1
        elapsed = time.time() - t0
        output = payload.get("result") or payload.get("error") or json.dumps(payload)
        status = payload.get("status", "unknown")
        path = OUT_DIR / f"{label.lower()}.md"
        path.write_text(str(output))
        evaluation = evaluate_output(output, phrase, status)
        evaluation["elapsed_s"] = round(elapsed, 1)
        evaluation["status"] = status
        evaluation["path"] = str(path)
        results[label] = evaluation
        print(
            f"  {label}: {elapsed:.1f}s score={evaluation['score']}/10 "
            f"cache_hit={evaluation['cache_hit']} chars={evaluation['chars']}",
            flush=True,
        )

    summary_path = OUT_DIR / "summary.json"
    summary_path.write_text(json.dumps(results, indent=2))
    print(json.dumps(results, indent=2))
    proc.terminate()
    failed = sum(1 for r in results.values() if r["failed"])
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
