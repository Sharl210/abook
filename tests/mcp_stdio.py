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


def start(argv=None):
    command = [BIN, "mcp", "stdio"] if argv is None else [BIN, *argv]
    return subprocess.Popen(
        command, stdin=subprocess.PIPE,
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
        d = send(p, request("d", "server/discover", add_meta=False))
        assert d["id"] == "d"
        dr = d["result"]
        assert dr["resultType"] == "complete"
        assert MODERN in dr["supportedVersions"]
        assert dr["_meta"]["io.modelcontextprotocol/serverInfo"]["name"] == "abook"
        assert "instructions" in dr

        tools = send(p, request("t", "tools/list"))["result"]
        assert tools["resultType"] == "complete"
        assert tools["ttlMs"] >= 0
        assert all(isinstance(x.get("inputSchema"), dict) for x in tools["tools"])
        names = [x["name"] for x in tools["tools"]]
        assert names == sorted(names)
        assert len(names) == len(set(names))
        assert "memory_index" in names
        assert "abook_route_--help" not in names
        assert "abook_route_-h" not in names
        assert "abook_txn_preview" in names
        assert "abook_txn_apply" in names
        assert "abook_txn_rollback" in names
        assert "abook_txn_status" in names
        assert "abook_migrate_scan" in names
        assert "abook_migrate_stage" in names
        assert "abook_migrate_activate" in names
        assert "abook_migrate_recover" in names
        assert "abook_migrate_rollback" in names
        helper = {x["name"]: x["inputSchema"] for x in tools["tools"] if x["name"] in {"memory_help", "spec_list", "spec_get", "skill_list", "skill_read", "system_update"}}
        assert "argv" not in helper["memory_help"]["properties"]
        assert helper["memory_help"]["additionalProperties"] is False
        assert helper["spec_get"]["required"] == ["name"]
        assert helper["skill_read"]["required"] == ["name"]
        assert "root" in helper["skill_read"]["properties"]
        registry = next(tool for tool in tools["tools"] if tool["name"] == "abook_attach_add")
        assert "command" not in registry["inputSchema"]["properties"]
        assert registry["inputSchema"]["additionalProperties"] is False

        resources = send(p, request("r", "resources/list"))["result"]
        assert resources["resultType"] == "complete"
        read_help = send(p, request("rr", "resources/read", {"uri": "abook://help"}))
        assert read_help["result"]["contents"][0]["text"]
        missing_resource = send(p, request("rr-missing", "resources/read", {"uri": "abook://spec/__does_not_exist__"}))
        assert missing_resource["error"]["code"] == -32602
        helper_unknown = send(p, request("helper-extra", "tools/call", {"name": "spec_list", "arguments": {"extra": True}}))
        assert helper_unknown["error"]["code"] == -32602
        helper_missing = send(p, request("helper-missing", "tools/call", {"name": "spec_get", "arguments": {}}))
        assert helper_missing["error"]["code"] == -32602
        skill_root = tempfile.mkdtemp(prefix="abook-mcp-skills-")
        try:
            os.makedirs(os.path.join(skill_root, "demo"), exist_ok=True)
            with open(os.path.join(skill_root, "demo", "SKILL.md"), "w", encoding="utf-8") as handle:
                handle.write("# Demo Skill\n\n正文 😀\n")
            skill_read = send(p, request("skill-read", "tools/call", {"name": "skill_read", "arguments": {"name": "demo", "root": skill_root}}))
            assert skill_read["result"]["isError"] is False, skill_read
            assert "正文 😀" in skill_read["result"]["structuredContent"]["stdout"]
        finally:
            shutil.rmtree(skill_root)
        templates = send(p, request("rt", "resources/templates/list"))["result"]
        assert templates["resultType"] == "complete"
        prompts = send(p, request("p", "prompts/list"))["result"]
        assert prompts["resultType"] == "complete"
        prompts_get = send(p, request("pg", "prompts/get", {"name": "memory-first"}))
        assert prompts_get["result"]["messages"][0]["content"]["text"]


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

        txn_root = os.path.join(root, "txn-library")
        os.makedirs(txn_root, exist_ok=True)
        body_file = os.path.join(root, "txn-body.txt")
        with open(body_file, "w", encoding="utf-8") as handle:
            handle.write("txn line one\ntxn line two 😀\n")
        txn_preview = send(p, request("t1", "tools/call", {"name": "abook_txn_preview", "arguments": {"root": txn_root, "path": "planes/explicit/records/txn.md", "body_file": body_file}}))
        assert "result" in txn_preview, txn_preview
        txn_data = txn_preview["result"]["structuredContent"]
        assert "token" in txn_data, txn_preview
        txn_token = txn_data["token"]
        txn_apply = send(p, request("t2", "tools/call", {"name": "abook_txn_apply", "arguments": {"root": txn_root, "preview_token": txn_token, "expect_revision": str(txn_data["revision"])}}))
        assert txn_apply["result"]["isError"] is False, txn_apply
        with open(os.path.join(txn_root, "planes/explicit/records/txn.md"), encoding="utf-8") as handle:
            assert handle.read() == "txn line one\ntxn line two 😀\n"
        txn_status = send(p, request("t3", "tools/call", {"name": "abook_txn_status", "arguments": {"root": txn_root}}))
        assert "result" in txn_status, txn_status
        txn_rollback = send(p, request("t4", "tools/call", {"name": "abook_txn_rollback", "arguments": {"root": txn_root, "preview_token": txn_token}}))
        assert txn_rollback["result"]["isError"] is False, txn_rollback
        assert not os.path.exists(os.path.join(txn_root, "planes/explicit/records/txn.md"))
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
        assert {"memory_init", "memory_context", "memory_index", "memory_export", "memory_write", "memory_read", "memory_list", "memory_search", "memory_recall", "memory_update", "memory_delete", "memory_restore", "abook_attach_add", "abook_attach_list", "abook_attach_get", "abook_attach_rm"} <= names
        memory_list_schema = next(tool for tool in tools if tool["name"] == "memory_list")["inputSchema"]
        assert memory_list_schema["required"] == ["root"]

        assert "all" in next(tool for tool in tools if tool["name"] == "memory_export")["inputSchema"]["properties"]["plane"]["enum"]
        empty_root = tempfile.mkdtemp(prefix="abook-mcp-empty-")
        empty_init = send(p, request("m0-init", "tools/call", {"name": "memory_init", "arguments": {"root": empty_root}}))
        assert empty_init["result"]["isError"] is False, empty_init
        empty_index = send(p, request("m0-index", "tools/call", {"name": "memory_index", "arguments": {"root": empty_root, "plane": "all"}}))
        assert empty_index["result"]["isError"] is False, empty_index
        assert empty_index["result"]["structuredContent"]["total"] == 0
        assert len(empty_index["result"]["structuredContent"]["planes"]) == 6
        written = send(p, request("m1", "tools/call", {"name": "memory_write", "arguments": {"root": root, "plane": "explicit", "title": "偏好", "read_when": "查偏好", "content": "第一行\n第二行 😀"}}))
        assert written["result"]["isError"] is False, written

        record = written["result"]["structuredContent"]
        extra = send(p, request("m1b", "tools/call", {"name": "memory_write", "arguments": {"root": root, "plane": "explicit", "title": "第二条", "read_when": "分页验收", "content": "另一条记录"}}))
        assert extra["result"]["isError"] is False, extra
        indexed = send(p, request("m1-index", "tools/call", {"name": "memory_index", "arguments": {"root": root, "plane": "all", "limit": 1, "offset": 0}}))
        assert indexed["result"]["isError"] is False, indexed
        assert indexed["result"]["structuredContent"]["format"] == "abook.memory.index.v1"
        assert indexed["result"]["structuredContent"]["total"] == 2
        assert len(indexed["result"]["structuredContent"]["planes"]) == 6
        listed_all = send(p, request("m1-list-all", "tools/call", {"name": "memory_list", "arguments": {"root": root, "limit": 10, "offset": 0}}))
        assert listed_all["result"]["isError"] is False, listed_all
        assert listed_all["result"]["structuredContent"]["total"] == 2
        bad_index_plane = send(p, request("m1-index-bad", "tools/call", {"name": "memory_index", "arguments": {"root": root, "plane": "bogus"}}))
        assert bad_index_plane["result"]["isError"] is True, bad_index_plane
        read = send(p, request("m2", "tools/call", {"name": "memory_read", "arguments": {"root": root, "id": record["id"]}}))
        assert read["result"]["structuredContent"]["content"] == "第一行\n第二行 😀"
        search = send(p, request("m3", "tools/call", {"name": "memory_search", "arguments": {"root": root, "query": "第二行", "plane": "explicit"}}))
        assert len(search["result"]["structuredContent"]["items"]) == 1
        assert search["result"]["structuredContent"]["total"] == 1
        context = send(p, request("m4", "tools/call", {"name": "memory_context", "arguments": {"root": root}}))
        assert context["result"]["structuredContent"]["planes"]["explicit"] == 2
        exported = send(p, request("m4b", "tools/call", {"name": "memory_export", "arguments": {"root": root}}))
        assert exported["result"]["isError"] is False, exported
        assert exported["result"]["structuredContent"]["format"] == "abook.memory.export.v1"
        assert exported["result"]["structuredContent"]["total"] == 2
        assert exported["result"]["structuredContent"]["credentials"] == "vault"
        assert len(exported["result"]["structuredContent"]["items"]) == 2
        explicit_export = send(p, request("m4c", "tools/call", {"name": "memory_export", "arguments": {"root": root, "plane": "explicit"}}))
        assert explicit_export["result"]["structuredContent"]["planes"]["explicit"] == 2
        assert explicit_export["result"]["structuredContent"]["total"] == 2
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
        add = send(p, request("m14", "tools/call", {"name": "abook_attach_add", "arguments": {"record": record["id"], "file": source, "name": "proof.txt", "root": root, "force": True}}))
        assert add["result"]["isError"] is False, add
        attach_list = send(p, request("m15", "tools/call", {"name": "abook_attach_list", "arguments": {"record": record["id"], "root": root}}))
        assert attach_list["result"]["isError"] is False, attach_list
        assert attach_list["result"]["structuredContent"]["count"] == 1
        attach_get = send(p, request("m16", "tools/call", {"name": "abook_attach_get", "arguments": {"record": record["id"], "attachment_name": "proof.txt", "root": root}}))
        assert attach_get["result"]["isError"] is False, attach_get
        assert attach_get["result"]["structuredContent"]["stdout"].splitlines()[0].endswith("-refs/proof.txt")
        attach_rm = send(p, request("m17", "tools/call", {"name": "abook_attach_rm", "arguments": {"record": record["id"], "attachment_name": "proof.txt", "root": root}}))
        assert attach_rm["result"]["isError"] is False, attach_rm

    finally:
        p.stdin.close()
        p.wait(timeout=5)
        shutil.rmtree(root)
    assert p.returncode == 0, p.stderr.read()
def test_migrate_tools():
    root = tempfile.mkdtemp(prefix="abook-mcp-migrate-")
    p = start()
    try:
        os.makedirs(os.path.join(root, "canonical", "records"), exist_ok=True)
        os.makedirs(os.path.join(root, "candidate", "records"), exist_ok=True)
        fixture = os.path.join(root, "canonical", "records", "C-20261106T000010-aaaaaaaa.md")
        with open(fixture, "w", encoding="utf-8") as handle:
            handle.write("""[CANONICAL_MEMORY]
ID=C-20261106T000010-aaaaaaaa
TITLE=Migration fixture
KIND=DECISION
READ_WHEN=CI migration
[/CANONICAL_MEMORY]
[MEMORY_BODY]
migration body
[/MEMORY_BODY]
""")
        scan = send(p, request("mig-scan", "tools/call", {"name": "abook_migrate_scan", "arguments": {"root": root}}))
        assert "result" in scan, scan
        staging = os.path.join(os.path.dirname(root), "abook-mcp-stage-" + os.path.basename(root))
        staged = send(p, request("mig-stage", "tools/call", {"name": "abook_migrate_stage", "arguments": {"root": root, "staging_root": staging}}))
        assert "result" in staged, staged
        assert staged["result"]["isError"] is False, staged
        assert os.path.isdir(staging)
        shutil.rmtree(staging)

        activation_root = tempfile.mkdtemp(prefix="abook-mcp-activate-")
        activation_stage = os.path.join(os.path.dirname(activation_root), "abook-mcp-activate-stage-" + os.path.basename(activation_root))
        backup_root = None
        abandoned_root = None
        try:
            os.makedirs(os.path.join(activation_root, "canonical", "records"), exist_ok=True)
            os.makedirs(os.path.join(activation_root, "candidate", "records"), exist_ok=True)
            legacy_record = os.path.join(activation_root, "canonical", "records", "C-20261106T000011-bbbbbbbb.md")
            with open(legacy_record, "w", encoding="utf-8") as handle:
                handle.write("""[CANONICAL_MEMORY]
ID=C-20261106T000011-bbbbbbbb
TITLE=Activate fixture
KIND=DECISION
READ_WHEN=CI activation
[/CANONICAL_MEMORY]
[MEMORY_BODY]
activation body
[/MEMORY_BODY]
""")
            activated = send(p, request("mig-activate", "tools/call", {"name": "abook_migrate_activate", "arguments": {"root": activation_root, "staging_root": activation_stage, "activate": True}}))
            assert "result" in activated, activated
            assert activated["result"]["isError"] is False, activated
            backup_root = activated["result"]["structuredContent"]["backup_root"]
            recovered = send(p, request("mig-recover", "tools/call", {"name": "abook_migrate_recover", "arguments": {"root": activation_root, "recover": True}}))
            assert "result" in recovered, recovered
            assert recovered["result"]["isError"] is True, recovered
            assert recovered["result"]["structuredContent"]["error"]["tag"] == "INTEGRITY"
            rolled_back = send(p, request("mig-rollback", "tools/call", {"name": "abook_migrate_rollback", "arguments": {"root": activation_root, "backup_root": backup_root, "rollback": True}}))
            assert "result" in rolled_back, rolled_back
            assert rolled_back["result"]["isError"] is False, rolled_back
            abandoned_root = rolled_back["result"]["structuredContent"]["abandoned_root"]
            assert os.path.isfile(legacy_record)
        finally:
            shutil.rmtree(activation_root)
            if os.path.isdir(activation_stage):
                shutil.rmtree(activation_stage)
            if backup_root and os.path.isdir(backup_root):
                shutil.rmtree(backup_root)
            if abandoned_root and os.path.isdir(abandoned_root):
                shutil.rmtree(abandoned_root)
    finally:
        p.stdin.close()
        p.wait(timeout=5)
        shutil.rmtree(root)
    assert p.returncode == 0, p.stderr.read()


def test_default_stdio_and_cli_prefix():
    p = start([])
    try:
        discovered = send(p, request("default", "server/discover", add_meta=False))
        assert MODERN in discovered["result"]["supportedVersions"]
    finally:
        p.stdin.close()
        p.wait(timeout=5)
    assert p.returncode == 0, p.stderr.read()

    root = tempfile.mkdtemp(prefix="abook-cli-prefix-")
    try:
        result = subprocess.run(
            [BIN, "cli", "memory", "write", "--root", root, "--plane", "explicit", "--title", "CLI prefix", "--content", "prefix body", "--json"],
            text=True, encoding="utf-8", capture_output=True, check=True,
        )
        value = json.loads(result.stdout)
        assert value["content"] == "prefix body"
        assert value["plane"] == "explicit"
        listed = subprocess.run([BIN, "list", "--root", root, "--plane", "explicit", "--json"], text=True, encoding="utf-8", capture_output=True, check=True)
        listing = json.loads(listed.stdout)
        assert listing["total"] == 1
        indexed = subprocess.run([BIN, "index", "--root", root, "--plane", "all", "--limit", "1", "--json"], text=True, encoding="utf-8", capture_output=True, check=True)
        index_value = json.loads(indexed.stdout)
        assert index_value["format"] == "abook.memory.index.v1"
        assert index_value["total"] == 1
        shown = subprocess.run([BIN, "get", value["id"], "TITLE", "--root", root, "--json"], text=True, encoding="utf-8", capture_output=True, check=True)
        assert json.loads(shown.stdout) == "CLI prefix"
        paged = subprocess.run([BIN, "list", "--root", root, "--plane", "explicit", "--page", "2", "--per-page", "1", "--json"], text=True, encoding="utf-8", capture_output=True, check=True)
        assert json.loads(paged.stdout)["offset"] == 1
        rejected = subprocess.run([BIN, "list", "unexpected", "--root", root, "--json"], text=True, encoding="utf-8", capture_output=True)
        assert rejected.returncode != 0

        old_root = tempfile.mkdtemp(prefix="abook-legacy-root-")
        mixed_root = tempfile.mkdtemp(prefix="abook-mixed-root-")
        absent_root = tempfile.mkdtemp(prefix="abook-memory-absent-no-side-effect-")
        shutil.rmtree(absent_root)
        try:
            old_body = os.path.join(old_root, "legacy-body.txt")
            with open(old_body, "w", encoding="utf-8") as handle:
                handle.write("legacy body")
            old = subprocess.run([BIN, "write", "--root", old_root, "--title", "Legacy", "--read-when", "legacy smoke", "--body-file", old_body, "--json"], text=True, encoding="utf-8", capture_output=True)
            assert old.returncode == 0, f"stdout={old.stdout}\nstderr={old.stderr}"
            assert os.path.isdir(os.path.join(old_root, "candidate", "records"))
            os.makedirs(os.path.join(mixed_root, "planes"), exist_ok=True)
            mixed = subprocess.run([BIN, "index", "--root", mixed_root, "--json"], text=True, encoding="utf-8", capture_output=True)
            assert mixed.returncode != 0
            absent = subprocess.run([BIN, "memory", "context", "--root", absent_root, "--json"], text=True, encoding="utf-8", capture_output=True)
            assert absent.returncode != 0
            assert not os.path.exists(absent_root)
        finally:
            shutil.rmtree(old_root)
            shutil.rmtree(mixed_root)
            if os.path.exists(absent_root):
                shutil.rmtree(absent_root)

    finally:
        shutil.rmtree(root)


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
    test_migrate_tools()
    test_default_stdio_and_cli_prefix()
    test_eof_and_stdout_purity()
    print("MCP stdio tests: 8 suites passed")
