"""Read-only integration checks for the local navigation UI/MCP backend."""
import argparse
import json
from urllib.parse import urlencode
from urllib.request import urlopen


def check(url):
    def call(args):
        with urlopen(url + '/api/explore?' + urlencode({'tool': 'navigation', 'args': json.dumps(args, ensure_ascii=False)}), timeout=120) as response:
            result = json.load(response)
        assert 'error' not in result, result
        return result

    role = call(dict(mode='graph', category='Roles', name='ИзменениеПодразделенияВОшибках'))
    repeated = [n for n in role['nodes'] if n[2] == 'Работа']
    assert len(repeated) > 1
    assert len({n[11] for n in repeated}) == len(repeated)
    assert all(n[12] and n[13] for n in repeated), 'owners must survive outside the scene'
    target = dict(category='Catalogs', name='ВариантыОтчетов', field='КлючевыеСлова')
    sizes = {}
    for layer in ['code', 'data', 'structure', 'forms', 'rights']:
        graph = call(dict(mode='graph', relationship_mode=layer, **target))
        assert all(graph['kinds'][e[2]][3] == layer for e in graph['edges']), layer
        assert all(n[8] <= 2 for n in graph['nodes'])
        sizes[layer] = len(graph['edges'])
        if layer == 'data':
            data_graph = graph
    rights = call(dict(mode='graph', relationship_mode='rights', include_denied_rights=False, **target))
    assert all(not rights['kinds'][e[2]][0].endswith(':false') for e in rights['edges'])
    assert data_graph['edges'], 'fixture must include a query usage'
    edge = data_graph['edges'][0]
    nodes = data_graph['nodes']
    proof = call(dict(mode='path', relationship_mode='data', **{'from': nodes[edge[0]][0], 'to': nodes[edge[1]][0]}))
    assert proof['found'] and proof['hops'] <= 2
    assert any(s.get('snippet') for step in proof['steps'] for s in step['evidence'])
    for budget in [2000, 4000, 16000]:
        context = call(dict(mode='context', max_chars=budget, limit=50, **target))
        encoded = json.dumps(context, ensure_ascii=False, separators=(',', ':'))
        assert len(encoded) <= budget, (budget, len(encoded))
        assert context['returned_chars'] == len(encoded)
        assert context['next_offset'] > 0
        if context['has_more']:
            page = call(dict(mode='context', max_chars=budget, limit=50, offset=context['next_offset'], **target))
            assert page['next_offset'] > context['next_offset']
    search = call(dict(mode='search', query='изменить ответственного за ошибку', kind='any', limit=10))
    assert search['found'] and search['search_mode']
    assert all(r[4] for r in search['rows']), 'search candidates must have exact addresses'
    print(json.dumps(dict(duplicate_names=len(repeated), layers=sizes, path_hops=proof['hops'], context_budgets=[2000,4000,16000], search_candidates=search['found'], search_mode=search['search_mode']), ensure_ascii=False))


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--url', default='http://127.0.0.1:8791')
    check(parser.parse_args().url)
