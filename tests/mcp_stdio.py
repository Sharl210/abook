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
        assert dr["supportedVersions"] == [MODERN]
        assert dr["_meta"]["io.modelcontextprotocol/serverInfo"]["name"] == "abook"
        assert "instructions" in dr
        assert "普通记忆平面" in dr["instructions"]
        assert "credentials" in dr["instructions"]
        assert "candidate/canonical" in dr["instructions"]
        # 两条优先级要求必须出现在主入口里，否则模型会退回命令行：
        # ① 有工具就用工具，不要用 CLI；② 环境没接 MCP 就先配 MCP 再干活。
        assert "优先用这里提供的工具" in dr["instructions"]
        assert "不要退回命令行" in dr["instructions"]
        assert "为本环境配置它" in dr["instructions"]
        # 配置期就要把库根写进启动参数，免得每次调用都传 root。
        assert "mcp stdio --root" in dr["instructions"]
        # 配法只给判断原则，不写死某个宿主的分步手册——写死就会误导别的环境。
        assert "不必套用别的宿主的写法" in dr["instructions"]
        discover_meta = send(p, request("d-meta", "server/discover"))
        assert discover_meta["result"]["resultType"] == "complete"
        assert discover_meta["result"]["supportedVersions"] == [MODERN]

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
        assert "abook_migrate_verify" in names
        assert "abook_commands" in names and "abook_describe" in names and "abook_help" in names
        assert "abook_version" in names and "abook_detect" in names and "abook_upgrade" in names
        for legacy_name in ["abook_write", "abook_add", "abook_update", "abook_delete", "abook_show", "abook_list", "abook_search", "abook_export", "abook_state", "abook_paths", "abook_branch", "abook_init", "abook_refresh", "abook_repair"]:
            assert legacy_name not in names, legacy_name
        hidden_call = send(p, request("legacy-hidden", "tools/call", {"name": "abook_write", "arguments": {"title": "legacy", "body": "must reject"}}))
        assert hidden_call["error"]["code"] == -32602, hidden_call
        for legacy_name in ["abook_write", "abook_add", "abook_update", "abook_delete", "abook_show", "abook_list", "abook_search", "abook_export", "abook_state", "abook_paths", "abook_branch", "abook_init", "abook_refresh", "abook_repair"]:
            assert legacy_name not in names, legacy_name
        hidden_call = send(p, request("legacy-hidden", "tools/call", {"name": "abook_write", "arguments": {"title": "legacy", "body": "must reject"}}))
        assert hidden_call["error"]["code"] == -32602, hidden_call
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
        assert "六平面结构化记忆" in read_help["result"]["contents"][0]["text"]
        assert "保险箱" in read_help["result"]["contents"][0]["text"]
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
        write_prompt = send(p, request("pgw", "prompts/get", {"name": "memory-write"}))
        assert "memory_write" in write_prompt["result"]["messages"][0]["content"]["text"]


        bad_meta = send(p, request("bm", "tools/list", add_meta=False))
        assert bad_meta["error"]["code"] == -32602
        bad_version = send(p, request("bv", "tools/list", version="1900-01-01"))
        assert bad_version["error"]["code"] == -32022
        bad_version = send(p, request("bv", "tools/list", version=LEGACY))
        assert bad_version["error"]["code"] == -32022
        assert bad_version["error"]["data"]["supported"] == [MODERN]
        bad_discover_version = send(p, request("bdv", "server/discover", version=LEGACY))
        assert bad_discover_version["error"]["code"] == -32022
        assert bad_discover_version["error"]["data"]["supported"] == [MODERN]
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
        init_modern = send(p, {
            "jsonrpc": "2.0", "id": "init-modern", "method": "initialize",
            "params": {"protocolVersion": MODERN, "capabilities": {}, "clientInfo": CLIENT},
        })
        assert init_modern["error"]["code"] == -32602
        init_old = send(p, {
            "jsonrpc": "2.0", "id": "init-old", "method": "initialize",
            "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": CLIENT},
        })
        assert init_old["error"]["code"] == -32602
        assert "server/discover" in init_old["error"]["message"]
        pre_initialized_tools = send(p, {"jsonrpc": "2.0", "id": "before-init", "method": "tools/list", "params": {}})
        assert pre_initialized_tools["error"]["code"] == -32602
        send(p, {"jsonrpc": "2.0", "method": "notifications/initialized"}, response=False)
        tools = send(p, {"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}})
        assert tools["id"] == 2
        assert "tools" in tools["result"]
        assert "resultType" not in tools["result"]
        legacy_discover = send(p, {"jsonrpc": "2.0", "id": "legacy-discover", "method": "server/discover", "params": {}})
        assert legacy_discover["error"]["code"] == -32601
    finally:
        p.stdin.close()
        p.wait(timeout=5)
    assert p.returncode == 0, p.stderr.read()


def test_batch_tools():
    root = tempfile.mkdtemp(prefix="abook-mcp-batch-")
    p = start()
    try:
        # 写进 `planes/<平面>/records/` 的正文必须是能被解析回来的记录。这不是
        # 测试偏好，而是数据完整性要求：records() 是 list/context/search/export
        # 的共同入口，一条无法解析的文件会让整库读不了。
        # 库要先初始化：下面要断言"写完之后整库仍可读"，而读路径要求六平面
        # 布局存在。不初始化就断言可读，测的是"未建库能不能读"，不是本意。
        init = send(p, request("b0", "tools/call", {"name": "memory_init", "arguments": {"root": root}}))
        assert init["result"]["isError"] is False, init
        record_body = (
            "[CANONICAL_MEMORY]\n"
            "ID=M-20260101T000000-batchdemo\n"
            "TITLE=批量写入示例\n"
            "READ_WHEN=验证批量工具时\n"
            "[/CANONICAL_MEMORY]\n"
            "[MEMORY_BODY]\n"
            "标题\n正文 😀\n"
            "[/MEMORY_BODY]\n"
        )
        preview = send(p, request("b1", "tools/call", {"name": "memory_batch_preview", "arguments": {"root": root, "path": "planes/explicit/records/demo.md", "body": record_body}}))
        result = preview["result"]
        assert result["isError"] is False, result
        token = result["structuredContent"]["token"]
        assert result["structuredContent"]["revision"] == 0
        # 非法正文必须被挡在 preview，不能等到写进去才让库读不了。
        bad = send(p, request("b1bad", "tools/call", {"name": "memory_batch_preview", "arguments": {"root": root, "path": "planes/explicit/records/bad.md", "body": "标题\n正文 😀\n"}}))
        assert bad["result"]["isError"] is True, bad
        assert not os.path.exists(os.path.join(root, "planes/explicit/records/bad.md"))
        status = send(p, request("b2", "tools/call", {"name": "memory_batch_status", "arguments": {"root": root}}))
        assert status["result"]["structuredContent"]["pending_transactions"] == 0
        applied = send(p, request("b3", "tools/call", {"name": "memory_batch_apply", "arguments": {"root": root, "token": token, "expected_revision": 0}}))
        assert applied["result"]["isError"] is False, applied
        with open(os.path.join(root, "planes/explicit/records/demo.md"), encoding="utf-8") as handle:
            assert handle.read() == record_body
        # 写完之后整库仍必须可读——这正是原缺陷的判别点。
        lst = send(p, request("b3list", "tools/call", {"name": "memory_list", "arguments": {"root": root, "plane": "explicit"}}))
        assert lst["result"]["isError"] is False, lst
        rolled = send(p, request("b4", "tools/call", {"name": "memory_batch_rollback", "arguments": {"root": root, "token": token}}))
        assert rolled["result"]["isError"] is False, rolled
        assert not os.path.exists(os.path.join(root, "planes/explicit/records/demo.md"))

        txn_root = os.path.join(root, "txn-library")
        os.makedirs(txn_root, exist_ok=True)
        txn_init = send(p, request("t0", "tools/call", {"name": "memory_init", "arguments": {"root": txn_root}}))
        assert txn_init["result"]["isError"] is False, txn_init
        body_file = os.path.join(root, "txn-body.txt")
        txn_body = (
            "[CANONICAL_MEMORY]\n"
            "ID=M-20260101T000001-txndemo\n"
            "TITLE=事务写入示例\n"
            "[/CANONICAL_MEMORY]\n"
            "[MEMORY_BODY]\n"
            "txn line one\ntxn line two 😀\n"
            "[/MEMORY_BODY]\n"
        )
        with open(body_file, "w", encoding="utf-8") as handle:
            handle.write(txn_body)
        txn_preview = send(p, request("t1", "tools/call", {"name": "abook_txn_preview", "arguments": {"root": txn_root, "path": "planes/explicit/records/txn.md", "body_file": body_file}}))
        assert "result" in txn_preview, txn_preview
        txn_data = txn_preview["result"]["structuredContent"]
        assert "token" in txn_data, txn_preview
        txn_token = txn_data["token"]
        txn_apply = send(p, request("t2", "tools/call", {"name": "abook_txn_apply", "arguments": {"root": txn_root, "preview_token": txn_token, "expect_revision": str(txn_data["revision"])}}))
        assert txn_apply["result"]["isError"] is False, txn_apply
        with open(os.path.join(txn_root, "planes/explicit/records/txn.md"), encoding="utf-8") as handle:
            assert handle.read() == txn_body
        txn_list = send(p, request("t2list", "tools/call", {"name": "memory_list", "arguments": {"root": txn_root, "plane": "explicit"}}))
        assert txn_list["result"]["isError"] is False, txn_list
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
        assert {"memory_init", "memory_context", "memory_paths", "memory_index", "memory_export", "memory_write", "memory_read", "memory_list", "memory_search", "memory_recall", "memory_update", "memory_delete", "memory_restore", "abook_attach_add", "abook_attach_list", "abook_attach_get", "abook_attach_rm"} <= names
        memory_list_schema = next(tool for tool in tools if tool["name"] == "memory_list")["inputSchema"]
        # root 不再是必填：二进制与记忆库同层，库根由程序自己推导，
        # 让调用方每次粘贴路径既啰嗦又容易指错目录。required 里不该再有 root。
        assert "root" not in memory_list_schema.get("required", []), memory_list_schema.get("required")
        assert "root" in memory_list_schema["properties"], "root 仍应可作为可选参数传入"

        assert "all" in next(tool for tool in tools if tool["name"] == "memory_export")["inputSchema"]["properties"]["plane"]["enum"]
        empty_root = tempfile.mkdtemp(prefix="abook-mcp-empty-")
        empty_init = send(p, request("m0-init", "tools/call", {"name": "memory_init", "arguments": {"root": empty_root}}))
        assert empty_init["result"]["isError"] is False, empty_init
        vault_probe = send(p, request("m0-vault", "tools/call", {"name": "abook_vault_put", "arguments": {"root": empty_root, "ident": "marker-regression", "purpose": "test only", "body": "dummy"}}))
        assert vault_probe["result"]["isError"] is False, vault_probe
        assert not os.path.exists(os.path.join(empty_root, ".numbers")), "vault write must not contaminate six-plane root"
        empty_index = send(p, request("m0-index", "tools/call", {"name": "memory_index", "arguments": {"root": empty_root, "plane": "all"}}))
        assert empty_index["result"]["isError"] is False, empty_index
        assert empty_index["result"]["structuredContent"]["total"] == 0
        assert len(empty_index["result"]["structuredContent"]["planes"]) == 6
        empty_paths = send(p, request("m0-paths", "tools/call", {"name": "memory_paths", "arguments": {"root": empty_root}}))
        assert empty_paths["result"]["isError"] is False, empty_paths
        assert empty_paths["result"]["structuredContent"]["format"] == "abook.memory.paths.v1"
        assert len(empty_paths["result"]["structuredContent"]["planes"]) == 6
        shutil.rmtree(empty_root)
        written = send(p, request("m1", "tools/call", {"name": "memory_write", "arguments": {"root": root, "plane": "explicit", "title": "偏好", "read_when": "查偏好", "content": "第一行\n第二行 😀"}}))
        assert written["result"]["isError"] is False, written

        routed_memory = send(p, request("m-route-write", "tools/call", {"name": "abook_memory", "arguments": {"subcommand": "write", "root": root, "plane": "explicit", "title": "route body", "read_when": "MCP top-level route", "body": "MCP line one\nMCP line two 😀"}}))
        assert routed_memory["result"]["isError"] is False, routed_memory
        routed_record = routed_memory["result"]["structuredContent"]
        assert routed_record["content"] == "MCP line one\nMCP line two 😀", routed_record
        marker_recall = subprocess.run([BIN, "recall", "第二行", "--body", "--root", root, "--json"], text=True, encoding="utf-8", capture_output=True)
        assert marker_recall.returncode == 0, marker_recall.stderr
        marker_recall_value = json.loads(marker_recall.stdout)
        assert marker_recall_value["total"] >= 1 and marker_recall_value["items"], marker_recall_value
        alias_write = subprocess.run([BIN, "add", "--root", root, "--plane", "explicit", "--title", "alias body", "--read-when", "alias parser", "--body", "alias line one\nalias line two 😀", "--json"], text=True, encoding="utf-8", capture_output=True)
        assert alias_write.returncode == 0, alias_write.stderr
        alias_record = json.loads(alias_write.stdout)
        assert alias_record["content"] == "alias line one\nalias line two 😀", alias_record

        record = written["result"]["structuredContent"]
        extra = send(p, request("m1b", "tools/call", {"name": "memory_write", "arguments": {"root": root, "plane": "explicit", "title": "第二条", "read_when": "分页验收", "content": "另一条记录"}}))
        assert extra["result"]["isError"] is False, extra
        indexed = send(p, request("m1-index", "tools/call", {"name": "memory_index", "arguments": {"root": root, "plane": "all", "limit": 1, "offset": 0}}))
        assert indexed["result"]["isError"] is False, indexed
        assert indexed["result"]["structuredContent"]["format"] == "abook.memory.index.v1"
        assert indexed["result"]["structuredContent"]["total"] == 4
        assert len(indexed["result"]["structuredContent"]["planes"]) == 6
        listed_all = send(p, request("m1-list-all", "tools/call", {"name": "memory_list", "arguments": {"root": root, "limit": 10, "offset": 0}}))
        assert listed_all["result"]["isError"] is False, listed_all
        assert listed_all["result"]["structuredContent"]["total"] == 4
        bad_index_plane = send(p, request("m1-index-bad", "tools/call", {"name": "memory_index", "arguments": {"root": root, "plane": "bogus"}}))
        assert bad_index_plane["result"]["isError"] is True, bad_index_plane
        read = send(p, request("m2", "tools/call", {"name": "memory_read", "arguments": {"root": root, "id": record["id"]}}))
        assert read["result"]["structuredContent"]["content"] == "第一行\n第二行 😀"
        search = send(p, request("m3", "tools/call", {"name": "memory_search", "arguments": {"root": root, "query": "第二行", "plane": "explicit"}}))
        assert len(search["result"]["structuredContent"]["items"]) == 1
        assert search["result"]["structuredContent"]["total"] == 1
        context = send(p, request("m4", "tools/call", {"name": "memory_context", "arguments": {"root": root}}))
        assert context["result"]["structuredContent"]["planes"]["explicit"] == 4
        exported = send(p, request("m4b", "tools/call", {"name": "memory_export", "arguments": {"root": root}}))
        assert exported["result"]["isError"] is False, exported
        assert exported["result"]["structuredContent"]["format"] == "abook.memory.export.v1"
        assert exported["result"]["structuredContent"]["total"] == 4
        assert exported["result"]["structuredContent"]["credentials"] == "vault"
        assert len(exported["result"]["structuredContent"]["items"]) == 4
        explicit_export = send(p, request("m4c", "tools/call", {"name": "memory_export", "arguments": {"root": root, "plane": "explicit"}}))
        assert explicit_export["result"]["structuredContent"]["planes"]["explicit"] == 4
        assert explicit_export["result"]["structuredContent"]["total"] == 4
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
        empty_attach_list = send(p, request("m18", "tools/call", {"name": "abook_attach_list", "arguments": {"record": record["id"], "root": root}}))
        assert empty_attach_list["result"]["isError"] is False, empty_attach_list
        assert empty_attach_list["result"]["structuredContent"]["count"] == 0
        assert empty_attach_list["result"]["structuredContent"]["items"] == []

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
            relation_target = os.path.join(activation_root, "candidate", "records", "N-20261106T000012-cccccccc.md")
            with open(relation_target, "w", encoding="utf-8") as handle:
                handle.write("""[CANDIDATE_MEMORY]
ID=N-20261106T000012-cccccccc
TITLE=Relation target
KIND=DECISION
[/CANDIDATE_MEMORY]
[MEMORY_BODY]
relation target body
[/MEMORY_BODY]
""")
            with open(legacy_record, "w", encoding="utf-8") as handle:
                handle.write("""[CANONICAL_MEMORY]
ID=C-20261106T000011-bbbbbbbb
TITLE=Activate fixture
KIND=DECISION
READ_WHEN=CI activation
RELATED_IDS=N-20261106T000012-cccccccc
[/CANONICAL_MEMORY]
[MEMORY_BODY]
activation body
[/MEMORY_BODY]
""")
            activated = send(p, request("mig-activate", "tools/call", {"name": "abook_migrate_activate", "arguments": {"root": activation_root, "staging_root": activation_stage, "activate": True}}))
            assert "result" in activated, activated
            assert activated["result"]["isError"] is False, activated
            backup_root = activated["result"]["structuredContent"]["backup_root"]
            verified = send(p, request("mig-verify", "tools/call", {"name": "abook_migrate_verify", "arguments": {"root": activation_root, "source_root": backup_root, "verify": True}}))
            assert verified["result"]["isError"] is False, verified
            assert verified["result"]["structuredContent"]["status"] == "PASS", verified
            assert verified["result"]["structuredContent"]["relations"] == 1, verified
            relation_file = os.path.join(activation_root, "relation_edges.jsonl")
            with open(relation_file, encoding="utf-8") as handle:
                relation_rows = [json.loads(line) for line in handle if line.strip()]
            assert len(relation_rows) == 1 and relation_rows[0]["status"] == "resolved", relation_rows
            with open(relation_file, "a", encoding="utf-8") as handle:
                handle.write("{}\n")
            tampered_relation = send(p, request("mig-verify-rel-tamper", "tools/call", {"name": "abook_migrate_verify", "arguments": {"root": activation_root, "source_root": backup_root, "verify": True}}))
            assert tampered_relation["result"]["isError"] is True, tampered_relation
            assert tampered_relation["result"]["structuredContent"]["error"]["tag"] == "INTEGRITY", tampered_relation
            with open(relation_file, "rb+") as handle:
                handle.truncate(os.path.getsize(relation_file) - 3)
            with open(relation_file, "r", encoding="utf-8") as handle:
                relation_row = json.loads(handle.readline())
            os.unlink(os.path.join(activation_root, "relation_edges.tsv"))
            missing_relation = send(p, request("mig-verify-rel-missing", "tools/call", {"name": "abook_migrate_verify", "arguments": {"root": activation_root, "source_root": backup_root, "verify": True}}))
            assert missing_relation["result"]["isError"] is True, missing_relation
            assert missing_relation["result"]["structuredContent"]["error"]["tag"] == "INTEGRITY", missing_relation
            with open(os.path.join(activation_root, "relation_edges.tsv"), "w", encoding="utf-8") as handle:
                handle.write("source_rel\tfield\tordinal\tvalue\tstatus\ttarget_rel\tcandidates\n")
                handle.write("{}\t{}\t{}\t{}\t{}\t{}\t{}\n".format(relation_row["source_rel"], relation_row["field"], relation_row["ordinal"], relation_row["value"], relation_row["status"], relation_row["target_rel"], ",".join(relation_row["candidates"])))
            restored = send(p, request("mig-verify-rel-restored", "tools/call", {"name": "abook_migrate_verify", "arguments": {"root": activation_root, "source_root": backup_root, "verify": True}}))
            assert restored["result"]["isError"] is False, restored
            assert restored["result"]["structuredContent"]["status"] == "PASS", restored
            assert restored["result"]["structuredContent"]["relations"] == 1, restored
            targets = []
            for base, _, files in os.walk(os.path.join(activation_root, "planes")):
                targets.extend(os.path.join(base, name) for name in files if name.endswith(".md"))
            assert targets, activation_root
            target = targets[0]
            with open(target, "a", encoding="utf-8") as handle:
                handle.write("drift")
            drift = send(p, request("mig-verify-drift", "tools/call", {"name": "abook_migrate_verify", "arguments": {"root": activation_root, "source_root": backup_root, "verify": True}}))
            assert drift["result"]["isError"] is True, drift
            assert drift["result"]["structuredContent"]["error"]["tag"] == "INTEGRITY", drift
            with open(target, "rb+") as handle:
                handle.truncate(os.path.getsize(target) - len(b"drift"))
            rogue = os.path.join(os.path.dirname(target), "not-in-manifest.md")
            with open(rogue, "w", encoding="utf-8") as handle:
                handle.write("extra record")
            extra = send(p, request("mig-verify-extra", "tools/call", {"name": "abook_migrate_verify", "arguments": {"root": activation_root, "source_root": backup_root, "verify": True}}))
            assert extra["result"]["isError"] is True, extra
            assert extra["result"]["structuredContent"]["error"]["tag"] == "INTEGRITY", extra
            os.unlink(rogue)
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
        assert discovered["result"]["supportedVersions"] == [MODERN]
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


def test_plane_layout_diagnostics():
    """六平面库上 reconcile/doctor 必须按六平面口径判健康。

    回归自真实缺陷：reconcile 固定遍历旧 canonical/candidate，在刚建好的六平面
    库上直接 IO 失败(rc=6)；doctor 固定查 Global.md/Index.md，把健康库判成异常
    (rc=5)；vault 又是按需创建的，把它算缺失等于给每个新库记一条假异常。
    """
    root = tempfile.mkdtemp(prefix="abook-mcp-diag-")
    p = start()
    try:
        init = send(p, request("g0", "tools/call", {"name": "memory_init", "arguments": {"root": root}}))
        assert init["result"]["isError"] is False, init
        w = send(p, request("g1", "tools/call", {"name": "memory_write", "arguments": {"root": root, "plane": "explicit", "title": "诊断样本", "content": "正文"}}))
        assert w["result"]["isError"] is False, w
        # reconcile/doctor 没有独立的 MCP 工具，直接跑 CLI：缺陷本来就在 CLI 层。
        for cmd in ("reconcile", "doctor"):
            done = subprocess.run([BIN, cmd, "--root", root, "--json"],
                                  capture_output=True, text=True)
            assert done.returncode == 0, (cmd, done.returncode, done.stdout, done.stderr)
    finally:
        p.stdin.close()
        p.wait(timeout=5)
        shutil.rmtree(root)
    assert p.returncode == 0, p.stderr.read()


def test_root_optional_and_field_edit():
    """root 可省略、头部字段可编辑、跨平面移动保留身份。

    三条对应真实使用里逼人退回 CLI 的摩擦：每次都要粘贴库路径、
    改不了 KIND/AUTHORITY 之类的头部字段、想把记录换平面只能删了重写。
    """
    root = tempfile.mkdtemp(prefix="abook-mcp-rootopt-")
    p = start()
    try:
        init = send(p, request("r0", "tools/call", {"name": "memory_init", "arguments": {"root": root}}))
        assert init["result"]["isError"] is False, init

        # 1) 省略 root：用 ABOOK_ROOT 指向同一库，验证省略时真的解析到它，
        #    而不是靠"看看报错文案里有没有 root"这种不稳的判据。
        explicit = send(p, request("r2", "tools/call", {"name": "memory_list", "arguments": {"root": root, "plane": "explicit"}}))
        assert explicit["result"]["isError"] is False, explicit
        p2 = subprocess.Popen(
            [BIN, "mcp", "stdio"], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            stderr=subprocess.PIPE, text=True, encoding="utf-8", bufsize=1,
            env={**os.environ, "ABOOK_ROOT": root},
        )
        try:
            send(p2, request("e0", "initialize", {"protocolVersion": LEGACY, "capabilities": CAPS, "clientInfo": CLIENT}))
            send(p2, request("e1", "notifications/initialized", {}, add_meta=False), response=False)
            no_root = send(p2, request("e2", "tools/call", {"name": "memory_list", "arguments": {"plane": "explicit"}}))
            assert no_root["result"]["isError"] is False, no_root
            # 与显式传 root 的结果一致，说明确实解析到同一个库。
            assert no_root["result"]["structuredContent"]["total"] == explicit["result"]["structuredContent"]["total"], (no_root, explicit)
        finally:
            p2.stdin.close()
            p2.wait(timeout=5)

        # 2) 头部字段编辑
        wrote = send(p, request("r3", "tools/call", {"name": "memory_write", "arguments": {"root": root, "plane": "long_term", "title": "字段编辑样本", "content": "正文"}}))
        assert wrote["result"]["isError"] is False, wrote
        rec_id = wrote["result"]["structuredContent"]["id"]
        upd = send(p, request("r4", "tools/call", {"name": "memory_update", "arguments": {
            "root": root, "id": rec_id, "expected_revision": 1,
            "fields": {"KIND": "DECISION", "AUTHORITY": "USER_EXPLICIT"}}}))
        assert upd["result"]["isError"] is False, upd
        read = send(p, request("r5", "tools/call", {"name": "memory_read", "arguments": {"root": root, "id": rec_id}}))
        body = json.dumps(read["result"]["structuredContent"], ensure_ascii=False)
        assert "DECISION" in body and "USER_EXPLICIT" in body, body[:400]

        # 3) 跨平面移动：ID 不变、平面改变、旧平面不再有它
        moved = send(p, request("r6", "tools/call", {"name": "memory_move_plane", "arguments": {
            "root": root, "id": rec_id, "to_plane": "profile", "expected_revision": 2}}))
        assert moved["result"]["isError"] is False, moved
        sc = moved["result"]["structuredContent"]
        assert sc["id"] == rec_id and sc["plane"] == "profile" and sc["from_plane"] == "long_term", sc
        old_plane = send(p, request("r7", "tools/call", {"name": "memory_list", "arguments": {"root": root, "plane": "long_term"}}))
        assert old_plane["result"]["structuredContent"]["total"] == 0, old_plane
        new_plane = send(p, request("r8", "tools/call", {"name": "memory_list", "arguments": {"root": root, "plane": "profile"}}))
        assert new_plane["result"]["structuredContent"]["total"] == 1, new_plane
    finally:
        p.stdin.close()
        p.wait(timeout=5)
        shutil.rmtree(root)
    assert p.returncode == 0, p.stderr.read()


def test_attach_root_at_config_time():
    """配置期把库根写进启动参数，之后每次调用都不必再传。

    这是 MCP 服务该有的形态：客户端在配置里写一次
    `args: ["mcp", "stdio", "--root", "/path"]`，进程内所有工具都用它。
    靠"每次调用时推导"不如这样确定——推导取决于二进制位置、环境变量和 cwd，
    任何一处变动都会指向别的库。
    """
    root = tempfile.mkdtemp(prefix="abook-mcp-attach-")
    try:
        setup = subprocess.run([BIN, "memory", "init", "--root", root], capture_output=True, text=True)
        assert setup.returncode == 0, setup.stderr
        p = start(["mcp", "stdio", "--root", root])
        try:
            send(p, request("a0", "initialize", {"protocolVersion": LEGACY, "capabilities": CAPS, "clientInfo": CLIENT}))
            send(p, request("a1", "notifications/initialized", {}, add_meta=False), response=False)
            # 完全不传 root
            wrote = send(p, request("a2", "tools/call", {"name": "memory_write", "arguments": {
                "plane": "explicit", "title": "配置期固定库根", "content": "正文"}}))
            assert wrote["result"]["isError"] is False, wrote
            ctx = send(p, request("a3", "tools/call", {"name": "memory_context", "arguments": {}}))
            assert ctx["result"]["isError"] is False, ctx
            assert ctx["result"]["structuredContent"]["total"] == 1, ctx
            # 写下来的确实在启动参数指定的那个库
            assert os.path.isdir(os.path.join(root, "planes", "explicit", "records")), "记录应落在 --root 指定的库"
            on_disk = os.listdir(os.path.join(root, "planes", "explicit", "records"))
            assert len(on_disk) == 1, on_disk
        finally:
            p.stdin.close()
            p.wait(timeout=5)
    finally:
        shutil.rmtree(root, ignore_errors=True)


def test_detect_sees_plane_layout():
    """`abook_detect` 必须认出六平面库。

    回归自一个真实缺陷：`memory init` 建出的新库只有 `planes/` 和
    `abook.memory.layout.v1`，没有旧布局的 `Global.md`/`Index.md`。detect
    原先只查旧文件，于是对刚建好的库回答 initialized:false，用户被告知
    "需要初始化"——而库其实好好的。判定口径与库真实布局脱节，属于产品
    自身缺陷，不是调用方用法问题。
    """
    root = tempfile.mkdtemp(prefix="abook-mcp-detect-")
    p = start()
    try:
        init = send(p, request("d-init", "tools/call", {"name": "memory_init", "arguments": {"root": root}}))
        assert init["result"]["isError"] is False, init
        got = send(p, request("d-detect", "tools/call", {"name": "abook_detect", "arguments": {"root": root}}))
        payload = got["result"].get("structuredContent")
        if payload is None:
            payload = json.loads(got["result"]["content"][0]["text"])
        assert payload["initialized"] is True, payload
        assert payload["root"] == root, payload
    finally:
        p.stdin.close()
        p.wait(timeout=5)
        shutil.rmtree(root)
    assert p.returncode == 0, p.stderr.read()


if __name__ == "__main__":
    test_modern()
    test_legacy()
    test_batch_tools()
    test_memory_tools()
    test_migrate_tools()
    test_detect_sees_plane_layout()
    test_plane_layout_diagnostics()
    test_root_optional_and_field_edit()
    test_attach_root_at_config_time()
    test_default_stdio_and_cli_prefix()
    test_eof_and_stdout_purity()
    print("MCP stdio tests: 12 suites passed")
