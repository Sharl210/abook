#!/usr/bin/env python3
"""Wire-level tests for the single-binary abook MCP stdio transport."""
import json
import subprocess
import sys

BIN = sys.argv[1] if len(sys.argv) > 1 else "./target/release/abook"
MODERN = "2026-07-28"
LEGACY = "2025-11-25"
CLIENT = {"name": "abook-mcp-test", "version": "1.0"}
CAPS = {}


def meta(version=MODERN):
    return {
        "io.modelcontextprotocol/protocolVersion": version,
        "io.modelcontextprotocol/clientInfo": CLIENT,
        "io.modelcontextprotocol/clientCapabilities": CAPS,
    }


def start():
    return subprocess.Popen(
        [BIN, "mcp", "stdio"], stdin=subprocess.PIPE,
        stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
        encoding="utf-8", bufsize=1,
    )


def send(proc, payload, response=True):
    proc.stdin.write(json.dumps(payload, ensure_ascii=False, separators=(",", ":")) + "\n")
    proc.stdin.flush()
    if not response:
        return None
    line = proc.stdout.readline()
    assert line, "MCP server closed stdout before replying"
    obj = json.loads(line)
    assert obj.get("jsonrpc") == "2.0", obj
    return obj


def request(i, method, params=None, version=MODERN, add_meta=True):
    params = dict(params or {})
    if add_meta:
        params["_meta"] = meta(version)
    return {"jsonrpc": "2.0", "id": i, "method": method, "params": params}


def test_modern():
    p = start()
    try:
        d = send(p, request("d", "server/discover"))
        assert d["id"] == "d"
        dr = d["result"]
        assert dr["resultType"] == "complete"
        assert MODERN in dr["supportedVersions"]
        assert dr["_meta"]["io.modelcontextprotocol/serverInfo"]["name"] == "abook"
        assert "instructions" in dr

        tools = send(p, request("t", "tools/list"))["result"]
        assert tools["resultType"] == "complete"
        assert tools["ttlMs"] >= 0
        names = [x["name"] for x in tools["tools"]]
        assert names == sorted(names)
        assert len(names) == len(set(names))
        assert all(isinstance(x.get("inputSchema"), dict) for x in tools["tools"])

        resources = send(p, request("r", "resources/list"))["result"]
        assert resources["resultType"] == "complete"
        templates = send(p, request("rt", "resources/templates/list"))["result"]
        assert templates["resultType"] == "complete"
        prompts = send(p, request("p", "prompts/list"))["result"]
        assert prompts["resultType"] == "complete"

        bad_meta = send(p, request("bm", "tools/list", add_meta=False))
        assert bad_meta["error"]["code"] == -32602
        bad_version = send(p, request("bv", "tools/list", version="1900-01-01"))
        assert bad_version["error"]["code"] == -32022
        unknown = send(p, request("u", "does/not/exist"))
        assert unknown["error"]["code"] == -32601
        bad_tool = send(p, request("bt", "tools/call", {"name": "abook_no_such_tool", "arguments": {}}))
        assert bad_tool["error"]["code"] == -32602

        # Notifications must not consume a response slot.
        send(p, request(None, "notifications/cancelled", {"requestId": "never"}), response=False)
        pong = send(p, request("ping", "ping"))
        assert pong["id"] == "ping"
    finally:
        p.stdin.close()
        p.wait(timeout=5)
    assert p.returncode == 0, p.stderr.read()


def test_legacy():
    p = start()
    try:
        init = send(p, {
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"protocolVersion": LEGACY, "capabilities": {}, "clientInfo": CLIENT},
        })
        assert init["result"]["protocolVersion"] == LEGACY
        assert "resultType" not in init["result"]
        send(p, {"jsonrpc": "2.0", "method": "notifications/initialized"}, response=False)
        tools = send(p, {"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}})
        assert tools["id"] == 2
        assert "tools" in tools["result"]
        assert "resultType" not in tools["result"]
    finally:
        p.stdin.close()
        p.wait(timeout=5)
    assert p.returncode == 0, p.stderr.read()


def test_eof_and_stdout_purity():
    p = start()
    p.stdin.write(json.dumps(request("d", "server/discover"), separators=(",", ":")) + "\n")
    p.stdin.close()
    lines = p.stdout.readlines()
    p.wait(timeout=5)
    assert p.returncode == 0
    assert len(lines) == 1
    for line in lines:
        json.loads(line)


if __name__ == "__main__":
    test_modern()
    test_legacy()
    test_eof_and_stdout_purity()
    print("MCP stdio tests: 3 suites passed")
