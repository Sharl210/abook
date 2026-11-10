#!/usr/bin/env python3
"""MCP 并发：一次发出一批 tools/call，确认每一条都拿到自己的响应。

串行处理时这些请求会排队，虽然结果也对，但一个慢调用会卡住后面所有请求。
真正的判据是 **响应行完整且不交错**：多个线程各写半条 JSON，客户端拼出来
就是坏数据。所以这里既要求数量对，也要求每行都能单独 parse。
"""
import json, subprocess, sys, threading

META = {
    "io.modelcontextprotocol/protocolVersion": "2026-07-28",
    "io.modelcontextprotocol/clientInfo": {"name": "conc", "version": "1"},
    "io.modelcontextprotocol/clientCapabilities": {},
}


def main(binary):
    root = sys.argv[2] if len(sys.argv) > 2 else None
    argv = [binary, "mcp", "stdio"]
    if root:
        argv += ["--root", root]
    proc = subprocess.Popen(argv, stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)

    n = 16
    for i in range(n):
        req = {"jsonrpc": "2.0", "id": str(i), "method": "tools/call",
               "params": {"name": "memory_context", "arguments": {}, "_meta": META}}
        proc.stdin.write(json.dumps(req) + "\n")
    proc.stdin.flush()

    got = {}
    for _ in range(n):
        line = proc.stdout.readline()
        assert line.strip(), "响应行缺失"
        obj = json.loads(line)          # 交错会在这里直接抛异常
        got[obj["id"]] = obj
    proc.terminate()

    assert len(got) == n, f"只收到 {len(got)}/{n} 条响应"
    for i in range(n):
        r = got[str(i)]["result"]
        assert r.get("isError") is False, r
    print(json.dumps({"status": "PASS", "responses": len(got), "interleaved": False}))


if __name__ == "__main__":
    main(sys.argv[1])
