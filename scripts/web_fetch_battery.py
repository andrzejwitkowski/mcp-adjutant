#!/usr/bin/env python3
"""Run web_fetch queries via MCP JSON-RPC, poll jobs, save outputs."""
import json
import os
import subprocess
import sys
import time
import uuid
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
BIN = REPO_ROOT / "target/release/mcp-adjutant"
OUT_DIR = Path("/tmp/web_fetch_battery")

QUERIES = [
    ("Q1_rust", "Tokio async Rust: when to use spawn, spawn_blocking, and channels"),
    ("Q2_python", "Python 3.12 typing best practices for dataclasses and Protocol"),
    ("Q3_db", "PostgreSQL partial indexes: when to use them and common pitfalls"),
    ("Q4_react", "React 19: use() hook vs useEffect for server data fetching"),
    ("Q5_docker", "Docker multi-stage build best practices for Rust static binaries"),
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


def extract_text(tool_result):
    chunks = []
    for block in tool_result.get("content", []):
        if block.get("type") == "text":
            chunks.append(block.get("text", ""))
    return "".join(chunks)


def main():
    OUT_DIR.mkdir(exist_ok=True)
    env = os.environ.copy()
    env.setdefault(
        "MCP_ADJUTANT_CONFIG",
        "/tmp/mcp-adjutant-web-fetch-config.json",
    )

    proc = subprocess.Popen(
        [str(BIN)],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        cwd=REPO_ROOT,
        env=env,
    )
    rid = 1
    mcp_call(
        proc,
        "initialize",
        {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {"name": "web_fetch_battery", "version": "0"},
        },
        rid,
    )
    rid += 1
    proc.stdin.write(
        json.dumps(
            {"jsonrpc": "2.0", "method": "notifications/initialized", "params": {}}
        )
        + "\n"
    )
    proc.stdin.flush()

    tools = mcp_call(proc, "tools/list", {}, rid)
    rid += 1
    tool_names = [t.get("name") for t in tools.get("tools", [])]
    if "web_fetch" not in tool_names:
        print("ERROR: web_fetch not in tools:", tool_names, file=sys.stderr)
        proc.terminate()
        return 1

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
        status = payload.get("status")
        output = payload.get("result") or payload.get("error") or json.dumps(payload)
        path = OUT_DIR / f"{label.lower()}.md"
        path.write_text(str(output))
        cache_hit = isinstance(output, str) and "[CACHE HIT]" in output
        failed = status == "failed"
        results[label] = {
            "phrase": phrase,
            "elapsed_s": round(elapsed, 1),
            "status": status,
            "cache_hit": cache_hit,
            "failed": failed,
            "chars": len(str(output)),
            "path": str(path),
        }
        print(
            f"  {label}: {elapsed:.1f}s status={status} cache_hit={cache_hit} chars={len(str(output))}",
            flush=True,
        )
        if failed:
            print(f"  error: {payload.get('error', '')[:300]}", flush=True)

    # cache hit probe: repeat Q1 paraphrase
    label = "Q6_cache"
    phrase = "Rust Tokio runtime spawn blocking channels guide"
    request_uuid = str(uuid.uuid4())
    print(f"=== {label}: web_fetch (cache probe) ===", flush=True)
    t0 = time.time()
    tool_call(
        proc,
        "web_fetch",
        {"search_phrase": phrase, "request_uuid": request_uuid},
        rid,
    )
    rid += 1
    payload = poll_job(proc, request_uuid, rid)
    rid += 1
    output = payload.get("result") or payload.get("error") or json.dumps(payload)
    cache_hit = isinstance(output, str) and "[CACHE HIT]" in output
    results[label] = {
        "phrase": phrase,
        "elapsed_s": round(time.time() - t0, 1),
        "status": payload.get("status"),
        "cache_hit": cache_hit,
        "failed": payload.get("status") == "failed",
        "chars": len(str(output)),
        "path": str(OUT_DIR / f"{label.lower()}.md"),
    }
    Path(results[label]["path"]).write_text(str(output))
    print(
        f"  {label}: {results[label]['elapsed_s']}s cache_hit={cache_hit}",
        flush=True,
    )

    summary_path = OUT_DIR / "summary.json"
    summary_path.write_text(json.dumps(results, indent=2))
    print(json.dumps(results, indent=2))
    proc.terminate()
    return 0


if __name__ == "__main__":
    sys.exit(main())
