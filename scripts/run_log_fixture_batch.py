#!/usr/bin/env python3
"""Invoke analyze_log via mcp-adjutant stdio and poll until terminal."""
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
    "tests/fixtures/logs/rust_compile.log",
    "tests/fixtures/logs/rust_panic.log",
    "tests/fixtures/logs/python_traceback.log",
    "tests/fixtures/logs/node_typeerror.log",
    "tests/fixtures/logs/noisy_ambiguous.log",
]


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
                    "clientInfo": {"name": "log-fixture-runner", "version": "1"},
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

    # accept job
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

    # read accept response (may be multiple lines)
    time.sleep(0.2)
    accepted = ""
    while True:
        chunk = proc.stdout.readline()
        if not chunk:
            break
        accepted += chunk
        if '"id":1' in chunk or '"id": 1' in chunk:
            break

    # poll
    for _ in range(120):
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
            return {
                "log_path": log_path,
                "request_uuid": req_uuid,
                "status": status.get("status"),
                "error": status.get("error"),
                "result": status.get("result"),
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
    json.dump(out, sys.stdout, indent=2)
    print(file=sys.stdout)


if __name__ == "__main__":
    main()
