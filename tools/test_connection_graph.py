"""Audit /api/graph against SQLite, including calls beyond MCP page limits.

Usage: python tools/test_connection_graph.py DB --url http://127.0.0.1:8791
Only reads the database and HTTP API; never touches configuration sources.
"""
import argparse
from collections import Counter, deque
import json
import sqlite3
import time
from urllib.parse import urlencode
from urllib.request import urlopen


def audit(db, url):
    conn = sqlite3.connect(f"file:{db}?mode=ro", uri=True)
    # Start from the procedure with most resolved incoming call sites.
    key, sites = conn.execute(
        "SELECT callee_key,count(*) FROM calls WHERE callee_key IS NOT NULL "
        "GROUP BY callee_key ORDER BY count(*) DESC LIMIT 1"
    ).fetchone()
    started = time.perf_counter()
    with urlopen(url + "/api/graph?" + urlencode({"args": json.dumps({"key": "method:" + key})}), timeout=120) as response:
        graph = json.load(response)
    assert "error" not in graph, graph
    assert graph["complete_in_index"] and not graph["truncated"]
    nodes, edges = graph["nodes"], graph["edges"]
    ids = {n[0]: i for i, n in enumerate(nodes)}
    assert len(ids) == len(nodes), "duplicate canonical nodes"
    adjacent = [[] for _ in nodes]
    actual_calls = Counter()
    for a, b, kind, file, line, occurrences in edges:
        assert 0 <= a < len(nodes) and 0 <= b < len(nodes)
        assert occurrences > 0
        assert file is None or 0 <= file < len(graph["files"])
        adjacent[a].append(b)
        adjacent[b].append(a)
        code = graph["kinds"][kind][0]
        if code in ("call", "notification"):
            actual_calls[(nodes[a][0], nodes[b][0])] += occurrences
    expected_calls = Counter()
    methods = {id: "method:" + path + "::" + name.lower() for id, path, name in conn.execute(
        "SELECT m.id,mo.rel_path,m.name FROM methods m JOIN modules mo ON mo.id=m.module_id"
    )}
    for caller, target in conn.execute("SELECT caller_id,callee_key FROM calls WHERE callee_key IS NOT NULL"):
        a, b = methods[caller], "method:" + target
        if a in ids or b in ids:
            assert a in ids and b in ids, ("missing reachable procedure", a, b)
            expected_calls[(a, b)] += 1
    assert actual_calls == expected_calls, ("calls differ", expected_calls - actual_calls, actual_calls - expected_calls)
    distance = [-1] * len(nodes)
    distance[graph["root"]] = 0
    queue = deque([graph["root"]])
    while queue:
        a = queue.popleft()
        for b in adjacent[a]:
            if distance[b] < 0:
                distance[b] = distance[a] + 1
                queue.append(b)
    assert all(d >= 0 for d in distance), "unreachable nodes included"
    assert distance == [n[8] for n in nodes], "incorrect shortest distances"
    # Containment must retain every method, including those not directly called.
    contains = {(a, b) for a, b, k, *_ in edges if graph["kinds"][k][0] == "contains_method"}
    for method, module in conn.execute("SELECT m.id,mo.rel_path FROM methods m JOIN modules mo ON mo.id=m.module_id"):
        parent = "module:" + module
        if parent in ids:
            assert methods[method] in ids
            assert (ids[parent], ids[methods[method]]) in contains
    rights = sum(e[5] for e in edges if graph["kinds"][e[2]][0].startswith("right:"))
    # Pilot component contains every role: each explicit true/false must survive.
    all_roles = conn.execute("SELECT DISTINCT role FROM navigation_rights").fetchall()
    if all("object:Roles:" + r.lower() in ids for r, in all_roles):
        assert rights == conn.execute("SELECT count(*) FROM navigation_rights").fetchone()[0]
    result = dict(nodes=len(nodes), edges=len(edges), depth=max(distance),
                  call_sites=sum(actual_calls.values()), high_degree_call_sites=sites,
                  explicit_rights=rights, seconds=round(time.perf_counter() - started, 3))
    print(json.dumps(result, ensure_ascii=False))
    return result


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("db")
    parser.add_argument("--url", default="http://127.0.0.1:8791")
    args = parser.parse_args()
    audit(args.db, args.url)
