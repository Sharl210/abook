import json, subprocess, tempfile, time, sys

BIN = sys.argv[1] if len(sys.argv) > 1 else '.artifacts/v1.8.147/abook-aarch64-linux-android'


def run_batch(root, n):
    p = subprocess.Popen([BIN, 'mcp'], stdin=subprocess.PIPE,
                         stdout=subprocess.PIPE, text=True)
    def send(x):
        p.stdin.write(json.dumps(x, ensure_ascii=False) + '\n')
        p.stdin.flush()
    def call(i, nm, a):
        send({'jsonrpc': '2.0', 'id': i, 'method': 'tools/call',
              'params': {'name': nm, 'arguments': a}})
        while True:
            x = json.loads(p.stdout.readline())
            if x.get('id') == i:
                return x.get('result', {})
    send({'jsonrpc': '2.0', 'id': 'i', 'method': 'initialize',
          'params': {'protocolVersion': '2025-11-25', 'capabilities': {},
                     'clientInfo': {'name': 'stress', 'version': '1'}}})
    json.loads(p.stdout.readline())
    send({'jsonrpc': '2.0', 'method': 'notifications/initialized', 'params': {}})
    call('init', 'memory_init', {'root': root})
    t0 = time.time()
    ok = 0
    for k in range(n):
        r = call('w%d' % k, 'memory_write',
                 {'root': root, 'plane': 'explicit',
                  'title': '批量%d' % k, 'content': '正文%d' % k})
        if r.get('isError') is False:
            ok += 1
    dt = time.time() - t0
    lst = call('l', 'memory_list',
               {'root': root, 'plane': 'explicit', 'limit': 1}).get('structuredContent', {})
    p.terminate()
    return ok, lst.get('total'), dt


for n in (10000,):
    root = tempfile.mkdtemp(prefix='stress%d-' % n)
    ok, total, dt = run_batch(root, n)
    print('n=%d: 成功=%d 库内total=%s 耗时=%.1fs' % (n, ok, total, dt))
