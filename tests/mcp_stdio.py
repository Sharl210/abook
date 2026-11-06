#!/usr/bin/env python3
"""Wire-level tests for the single-binary abook MCP stdio transport."""
import json
import os
import shutil
import subprocess
import sys
import tempfile

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


def test_batch_tools():
    root = tempfile.mkdtemp(prefix="abook-mcp-batch-")
    p = start()
    try:
        preview = send(p, request("b1", "tools/call", {"name": "memory_batch_preview", "arguments": {"root": root, "path": "planes/explicit/records/demo.md", "body": "标题\n正文 😀\n"}}))
        result = preview["result"]
        assert result["isError"] is False, result
        token = result["structuredContent"]["token"]
        assert result["structuredContent"]["revision"] == 0
        status = send(p, request("b2", "tools/call", {"name": "memory_batch_status", "arguments": {"root": root}}))
        assert status["result"]["structuredContent"]["pending_transactions"] == 0
        applied = send(p, request("b3", "tools/call", {"name": "memory_batch_apply", "arguments": {"root": root, "token": token, "expected_revision": 0}}))
        assert applied["result"]["isError"] is False, applied
        with open(os.path.join(root, "planes/explicit/records/demo.md"), encoding="utf-8") as handle:
            assert handle.read() == "标题\n正文 😀\n"
        rolled = send(p, request("b4", "tools/call", {"name": "memory_batch_rollback", "arguments": {"root": root, "token": token}}))
        assert rolled["result"]["isError"] is False, rolled
        assert not os.path.exists(os.path.join(root, "planes/explicit/records/demo.md"))
        rejected = send(p, request("b5", "tools/call", {"name": "memory_batch_preview", "arguments": {"root": root, "path": "x", "body": "ok", "extra": "reject"}}))
        assert rejected["result"]["isError"] is True
    finally:
        p.stdin.close()
        p.wait(timeout=5)
        shutil.rmtree(root)
    assert p.returncode == 0, p.stderr.read()
def test_memory_tools():
    root = tempfile.mkdtemp(prefix="abook-mcp-memory-")
    p = start()
    try:
        tools = send(p, request("mt", "tools/list"))["result"]["tools"]
        names = {tool["name"] for tool in tools}
        assert {"memory_context", "memory_write", "memory_read", "memory_list", "memory_search", "memory_recall", "memory_update", "memory_delete", "memory_restore", "abook_attach_add", "abook_attach_list", "abook_attach_get", "abook_attach_rm"} <= names
        written = send(p, request("m1", "tools/call", {"name": "memory_write", "arguments": {"root": root, "plane": "explicit", "title": "偏好", "read_when": "查偏好", "content": "第一行\n第二行 😀"}}))
        assert written["result"]["isError"] is False, written
        record = written["result"]["structuredContent"]
        extra = send(p, request("m1b", "tools/call", {"name": "memory_write", "arguments": {"root": root, "plane": "explicit", "title": "第二条", "read_when": "分页验收", "content": "另一条记录"}}))
        assert extra["result"]["isError"] is False, extra
        read = send(p, request("m2", "tools/call", {"name": "memory_read", "arguments": {"root": root, "id": record["id"]}}))
        assert read["result"]["structuredContent"]["content"] == "第一行\n第二行 😀"
        search = send(p, request("m3", "tools/call", {"name": "memory_search", "arguments": {"root": root, "query": "第二行", "plane": "explicit"}}))
        assert len(search["result"]["structuredContent"]["items"]) == 1
        assert search["result"]["structuredContent"]["total"] == 1
        context = send(p, request("m4", "tools/call", {"name": "memory_context", "arguments": {"root": root}}))
        assert context["result"]["structuredContent"]["planes"]["explicit"] == 2
        revision = str(record["revision"])
        updated = send(p, request("m7", "tools/call", {"name": "memory_update", "arguments": {"root": root, "id": record["id"], "expected_revision": int(revision), "body": "更新后的正文"}}))
        assert updated["result"]["isError"] is False, updated
        assert updated["result"]["structuredContent"]["content"] == "更新后的正文"
        deleted = send(p, request("m8", "tools/call", {"name": "memory_delete", "arguments": {"root": root, "id": record["id"], "expected_revision": int(updated["result"]["structuredContent"]["revision"])}}))
        assert deleted["result"]["structuredContent"]["status"] == "deleted"
        restored = send(p, request("m9", "tools/call", {"name": "memory_restore", "arguments": {"root": root, "id": record["id"], "expected_revision": int(deleted["result"]["structuredContent"]["revision"])}}))
        assert restored["result"]["structuredContent"]["status"] == "active"
        stale = send(p, request("m10", "tools/call", {"name": "memory_update", "arguments": {"root": root, "id": record["id"], "expected_revision": 1, "content": "冲突"}}))
        assert stale["result"]["isError"] is True
        rejected = send(p, request("m11", "tools/call", {"name": "memory_write", "arguments": {"root": root, "plane": "credentials", "title": "x", "content": "y"}}))
        assert rejected["result"]["isError"] is True
        unknown = send(p, request("m12", "tools/call", {"name": "memory_search", "arguments": {"root": root, "query": "第二行", "unexpected": True}}))
        assert unknown["result"]["isError"] is True
        source = os.path.join(root, "attachment-source.txt")
        with open(source, "w", encoding="utf-8") as handle:
            handle.write("attachment bytes")
        add = send(p, request("m14", "tools/call", {"name": "abook_attach_add", "arguments": {"command": "attach add", "record": record["id"], "file": source, "name": "proof.txt", "root": root, "force": True}}))
        assert add["result"]["isError"] is False, add
        attach_list = send(p, request("m15", "tools/call", {"name": "abook_attach_list", "arguments": {"command": "attach list", "record": record["id"], "root": root}}))
        assert attach_list["result"]["isError"] is False, attach_list
        assert attach_list["result"]["structuredContent"]["count"] == 1
        attach_get = send(p, request("m16", "tools/call", {"name": "abook_attach_get", "arguments": {"command": "attach get", "record": record["id"], "attachment_name": "proof.txt", "root": root}}))
        assert attach_get["result"]["isError"] is False, attach_get
        assert attach_get["result"]["structuredContent"]["stdout"].splitlines()[0].endswith("-refs/proof.txt")
        attach_rm = send(p, request("m17", "tools/call", {"name": "abook_attach_rm", "arguments": {"command": "attach rm", "record": record["id"], "attachment_name": "proof.txt", "root": root}}))
        assert attach_rm["result"]["isError"] is False, attach_rm

    finally:
        p.stdin.close()
        p.wait(timeout=5)
        shutil.rmtree(root)
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
    test_batch_tools()
    test_memory_tools()
    test_eof_and_stdout_purity()
    print("MCP stdio tests: 5 suites passed")
