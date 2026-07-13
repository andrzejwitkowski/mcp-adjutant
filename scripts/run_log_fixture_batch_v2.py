#!/usr/bin/env python3
"""Invoke analyze_log on v2 fixture set via mcp-adjutant stdio."""
import json
import os
import subprocess
import sys
import time
import uuid

ROOT = os.environ.get(
    "MCP_ADJUTANT_PROJECT_ROOT",
    os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
)
BIN = os.path.join(ROOT, "target/release/mcp-adjutant")

LOGS = [
    "tests/fixtures/logs/v2/rust_quoted_panic.log",
    "tests/fixtures/logs/v2/python_keyerror.log",
    "tests/fixtures/logs/v2/node_reference_error.log",
    "tests/fixtures/logs/v2/java_null_pointer.log",
    "tests/fixtures/logs/v2/ci_buried_compile.log",
    "tests/fixtures/logs/v2/ambiguous_ops.log",
]

EXPECTED = {
    "rust_quoted_panic.log": {
        "error_type": "Panic",
        "target_file": "src/storage.rs",
        "line_number": 201,
        "column_number": 9,
    },
    "python_keyerror.log": {
        "error_type": "KeyError",
        "target_file": "lib/handlers.py",
        "line_number": 41,
    },
    "node_reference_error.log": {
        "error_type": "ReferenceError",
        "target_file": "frontend/src/modules/config-ui/types.ts",
        "line_number": 88,
        "column_number": 5,
    },
    "java_null_pointer.log": {
        "error_type": "NullPointerException",
        "target_file": "App.java",
        "line_number": 14,
    },
    "ci_buried_compile.log": {
        "error_type": "CompileError",
        "target_file": "src/mcp/handlers.rs",
        "line_number": 512,
        "column_number": 17,
    },
    "ambiguous_ops.log": {
        "error_type": "Unknown",
        "note": "low confidence; expect parser best-effort or llm_fallback_error",
    },
}


def rpc_lines(*calls):
    lines = [
        json.dumps(
            {
                "jsonrpc": "2.0",
                "id": 0,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2024-11-05",
                    "capabilities": {},
                    "clientInfo": {"name": "log-fixture-runner-v2", "version": "1"},
                },
            }
        )
    ]
    for i, call in enumerate(calls, start=1):
        lines.append(json.dumps({"jsonrpc": "2.0", "id": i, **call}))
    return "\n".join(lines) + "\n"


def parse_last_result(stdout: str) -> dict:
    for line in reversed(stdout.strip().splitlines()):
        line = line.strip()
        if not line.startswith("{"):
            continue
        try:
            msg = json.loads(line)
        except json.JSONDecodeError:
            continue
        if msg.get("result"):
            return msg
    raise RuntimeError(f"no JSON-RPC result in output:\n{stdout[-2000:]}")


def extract_text(result_msg: dict) -> str:
    content = result_msg.get("result", {}).get("content", [])
    if content and content[0].get("text"):
        return content[0]["text"]
    return json.dumps(result_msg)


def run_analyze(log_path: str) -> dict:
    req_uuid = str(uuid.uuid4())
    env = {**os.environ, "MCP_ADJUTANT_PROJECT_ROOT": ROOT}
    proc = subprocess.Popen(
        [BIN],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
        text=True,
        env=env,
    )
    assert proc.stdin and proc.stdout

    stdin_payload = rpc_lines(
        {
            "method": "tools/call",
            "params": {
                "name": "analyze_log",
                "arguments": {"log_path": log_path, "request_uuid": req_uuid},
            },
        }
    )
    proc.stdin.write(stdin_payload)
    proc.stdin.flush()

    time.sleep(0.2)
    while True:
        chunk = proc.stdout.readline()
        if not chunk:
            break
        if '"id":1' in chunk or '"id": 1' in chunk:
            break

    for _ in range(180):
        poll_payload = rpc_lines(
            {
                "method": "tools/call",
                "params": {
                    "name": "query_job_status",
                    "arguments": {"request_uuid": req_uuid},
                },
            }
        )
        proc.stdin.write(poll_payload)
        proc.stdin.flush()
        time.sleep(0.5)
        poll_out = ""
        while True:
            chunk = proc.stdout.readline()
            if not chunk:
                break
            poll_out += chunk
            if '"id":1' in chunk or '"id": 1' in chunk:
                break
        text = extract_text(parse_last_result(poll_out))
        try:
            status = json.loads(text)
        except json.JSONDecodeError:
            continue
        if status.get("terminal"):
            proc.terminate()
            name = os.path.basename(log_path)
            parsed = None
            if status.get("status") == "completed" and status.get("result"):
                try:
                    parsed = json.loads(status["result"])
                except json.JSONDecodeError:
                    parsed = {"raw": status["result"]}
            return {
                "log_path": log_path,
                "expected": EXPECTED.get(name, {}),
                "request_uuid": req_uuid,
                "status": status.get("status"),
                "error": status.get("error"),
                "report": parsed,
            }

    proc.terminate()
    raise TimeoutError(f"job {req_uuid} for {log_path} did not finish")


def main():
    if not os.path.isfile(BIN):
        print(f"missing binary: {BIN}", file=sys.stderr)
        sys.exit(1)
    out = []
    for log in LOGS:
        print(f"running analyze_log on {log}...", file=sys.stderr)
        out.append(run_analyze(log))
    json.dump({"fixtures": out, "expected": EXPECTED}, sys.stdout, indent=2)
    print(file=sys.stdout)


if __name__ == "__main__":
    main()
