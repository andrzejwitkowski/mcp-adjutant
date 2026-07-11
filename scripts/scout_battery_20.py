#!/usr/bin/env python3
"""Run 20 scout_context queries; evaluate each immediately after."""
import json
import re
import subprocess
import sys
import time
import uuid
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
BIN = REPO_ROOT / "target/release/mcp-adjutant"
OUT_DIR = Path("/tmp/scout_battery_20")

QUERIES = [
    ("S01", "How does web fetch cache store reports and sources in SQLite?"),
    ("S02", "Where is the Brave Search API called for web_fetch?"),
    ("S03", "How does run_scout_with_cache decide cache hit vs fresh run?"),
    ("S04", "Where is semantic similarity threshold defined for cache lookups?"),
    ("S05", "How does evaluate_agent_performance score agent output?"),
    ("S06", "Where is the config UI HTTP server started and on which port?"),
    ("S07", "How does JobRegistry track async MCP tool jobs?"),
    ("S08", "Where does ScoutAgent record ripgrep hits for cache dependencies?"),
    ("S09", "How does the transformer agent find refactor targets?"),
    ("S10", "Where is LocalEmbeddingEngine used for vector embeddings?"),
    ("S11", "How does verify_and_triage compile and fix code?"),
    ("S12", "Where are MCP tool handlers registered and dispatched?"),
    ("S13", "How does cache invalidation detect dirty code nodes?"),
    ("S14", "Where is the web fetcher agent orchestrator loop implemented?"),
    ("S15", "How does store_web_report link reports to fetched URLs?"),
    ("S16", "Where is AdjutantConfig loaded, migrated, and saved?"),
    ("S17", "How does ast_calls find physical call sites in source files?"),
    ("S18", "Where is scout and web cache pagination implemented for the config UI?"),
    ("S19", "How does dispatch_async_job run background MCP work?"),
    ("S20", "Where is web_cache_threshold configured and applied?"),
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


def heuristic_eval(query: str, output: str, status: str) -> dict:
    text = str(output)
    failed = status == "failed"
    cache_hit = "[CACHE HIT]" in text
    lower = text.lower()
    q_tokens = {t for t in re.findall(r"[a-zA-Z_]{4,}", query.lower())}
    matched = sum(1 for t in q_tokens if t in lower)
    has_paths = ".rs" in text or "src/" in text
    has_structure = "##" in text or "- " in text or "`" in text
    substantive = len(text.strip()) > 150 and not failed

    score = 0
    notes = []
    if failed:
        notes.append("job failed")
    else:
        if substantive:
            score += 3
            notes.append("substantive")
        if has_paths:
            score += 3
            notes.append(" cites file paths")
        if has_structure:
            score += 2
            notes.append("structured markdown")
        if matched >= 2:
            score += 2
            notes.append(f"query terms matched ({matched})")
        if cache_hit:
            notes.append("cache hit")

    return {
        "heuristic_score": min(score, 10),
        "cache_hit": cache_hit,
        "failed": failed,
        "chars": len(text),
        "notes": notes,
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
            "clientInfo": {"name": "scout_battery_20", "version": "0"},
        },
        rid,
    )
    rid += 1
    proc.stdin.write(
        json.dumps({"jsonrpc": "2.0", "method": "notifications/initialized", "params": {}})
        + "\n"
    )
    proc.stdin.flush()

    summary = {}
    for label, query in QUERIES:
        print(f"\n=== {label}: scout_context ===", flush=True)
        request_uuid = str(uuid.uuid4())
        t0 = time.time()
        tool_call(
            proc,
            "scout_context",
            {"query": query, "request_uuid": request_uuid, "force_refresh": True},
            rid,
        )
        rid += 1
        payload = poll_job(proc, request_uuid, rid)
        rid += 1
        scout_elapsed = time.time() - t0
        output = payload.get("result") or payload.get("error") or json.dumps(payload)
        status = payload.get("status", "unknown")
        scout_path = OUT_DIR / f"{label.lower()}_scout.txt"
        scout_path.write_text(str(output))

        heuristic = heuristic_eval(query, output, status)
        print(
            f"  scout: {scout_elapsed:.1f}s status={status} heuristic={heuristic['heuristic_score']}/10 "
            f"cache_hit={heuristic['cache_hit']} chars={heuristic['chars']}",
            flush=True,
        )

        print(f"=== {label}: evaluate_agent_performance ===", flush=True)
        eval_uuid = str(uuid.uuid4())
        t1 = time.time()
        tool_call(
            proc,
            "evaluate_agent_performance",
            {
                "target_agent": "Phase_1_Scout",
                "original_task": query,
                "received_output": str(output)[:12000],
                "request_uuid": eval_uuid,
            },
            rid,
        )
        rid += 1
        eval_payload = poll_job(proc, eval_uuid, rid, timeout=180)
        rid += 1
        eval_elapsed = time.time() - t1
        eval_text = eval_payload.get("result") or eval_payload.get("error") or json.dumps(eval_payload)
        eval_path = OUT_DIR / f"{label.lower()}_eval.txt"
        eval_path.write_text(str(eval_text))

        llm_score = None
        score_match = re.search(r'"score"\s*:\s*(\d+)', str(eval_text))
        if score_match:
            llm_score = int(score_match.group(1))
        elif re.search(r"\b(\d+)\s*/\s*10\b", str(eval_text)):
            llm_score = int(re.search(r"\b(\d+)\s*/\s*10\b", str(eval_text)).group(1))

        print(
            f"  eval: {eval_elapsed:.1f}s llm_score={llm_score} preview={str(eval_text)[:120]}...",
            flush=True,
        )

        summary[label] = {
            "query": query,
            "scout_elapsed_s": round(scout_elapsed, 1),
            "eval_elapsed_s": round(eval_elapsed, 1),
            "scout_status": status,
            "heuristic_score": heuristic["heuristic_score"],
            "llm_score": llm_score,
            "cache_hit": heuristic["cache_hit"],
            "scout_chars": heuristic["chars"],
            "heuristic_notes": heuristic["notes"],
            "scout_path": str(scout_path),
            "eval_path": str(eval_path),
        }

    (OUT_DIR / "summary.json").write_text(json.dumps(summary, indent=2))
    avg_heuristic = sum(v["heuristic_score"] for v in summary.values()) / len(summary)
    llm_scores = [v["llm_score"] for v in summary.values() if v["llm_score"] is not None]
    avg_llm = sum(llm_scores) / len(llm_scores) if llm_scores else None
    print(f"\n=== DONE: heuristic avg={avg_heuristic:.1f}/10 llm avg={avg_llm} ===", flush=True)
    print(json.dumps(summary, indent=2))
    proc.terminate()
    failed = sum(1 for v in summary.values() if v["scout_status"] == "failed")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
