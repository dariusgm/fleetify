"""Repeats one fixture on one prompt variant, to check that a confidence threshold is reliable.

Usage: python3 rerun_honest.py FIXTURE VARIANT REPS OUT.json
"""
import json, pathlib, statistics, sys
here = pathlib.Path(__file__).resolve().parent
src = (here / 'grid.py').read_text(encoding='utf-8').split('labels = json.load')[0]
ns = {'__file__': str(here / 'grid.py')}
exec(compile(src, 'grid.py', 'exec'), ns)
fixture, variant, reps, out = sys.argv[1], sys.argv[2], int(sys.argv[3]), sys.argv[4]
instr, crit = ns['VARIANTS'][variant]
state = (here / f'{fixture}.md').read_text(encoding='utf-8')
rows = []
for rep in range(reps):
    a = ns['call'](ns['body'](instr, crit, state))['answers']['verdict']
    rows.append({'rep': rep, 'choice': a.get('choice'), 'confidence': a.get('confidence')})
    print(rep, a.get('choice'), a.get('confidence'), flush=True)
json.dump(rows, open(out, 'w'), indent=1)
conf = [r['confidence'] for r in rows]
print('choices', {c: sum(r['choice'] == c for r in rows) for c in ('pass', 'fail', 'unsure')})
print('confidence min/mean/max', min(conf), round(statistics.mean(conf), 3), max(conf))
