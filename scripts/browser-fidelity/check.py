#!/usr/bin/env python3
"""The browser fidelity check's steps and verdict.

  check.py steps <pages dir> <out dir>      writes <out>/steps.json for YH_BROWSER_PROBE
  check.py report <out dir> <run>...        compares each run's log and shots with the reference
"""
import json, os, re, subprocess, sys

def steps(pages, out):
    sizes = json.load(open(f'{pages}/sizes.json'))
    metrics = open(f'{pages}/metrics.js').read().strip()
    layout, inputs = f'file://{pages}/layout.html', f'file://{pages}/input.html'
    measure = lambda label: {'tool': 'evaluate', 'args': {'expression': f"'M {label} ' + {metrics}"}}
    steps, shots = [{'tool': 'open', 'args': {'url': layout}}, {'sleep': 0.5}], {}
    def shot(name, **args):
        shots[name] = len(steps)
        steps.append({'tool': 'screenshot', 'args': args})
    for name, (w, h) in sizes.items():
        preset = {'preset': name} if name != 'custom' else {'width': w, 'height': h}
        steps += [{'tool': 'resize', 'args': preset}, {'tool': 'navigate', 'args': {'url': f'{layout}?{name}'}}, {'sleep': 0.6}, measure(name)]
        shot(f'{name}-view')
        shot(f'{name}-full', full_page=True)
    # Resized after loading, not reloaded.
    steps += [{'tool': 'resize', 'args': {'preset': 'laptop'}}, {'sleep': 0.6}, measure('resized-laptop')]
    # A full-page shot from part way down: the place comes back, the layout is as it was.
    steps += [{'tool': 'evaluate', 'args': {'expression': 'scrollTo(0, 1500); scrollY'}}]
    shot('scrolled-full', full_page=True)
    steps += [{'sleep': 0.4}, {'tool': 'evaluate', 'args': {'expression': "'S ' + JSON.stringify({y: scrollY, hero: document.getElementById('hero').offsetHeight, inner: innerHeight})"}}]
    # A phone-sized page without width=device-width gets a note.
    steps += [{'tool': 'resize', 'args': {'preset': 'phone'}},
              {'tool': 'evaluate', 'args': {'expression': "document.querySelector('meta[name=viewport]').remove(); 0"}}]
    shot('no-viewport-meta')
    # Input.
    state = ("JSON.stringify({h: getComputedStyle(document.getElementById('h')).backgroundColor, menu: getComputedStyle(document.getElementById('sub')).display, "
             "log: log.splice(0).join(' | '), seen, scroll: scrollY, selection: String(getSelection()), card: document.getElementById('card').parentElement.id})")
    report = lambda label: {'tool': 'evaluate', 'args': {'expression': f"'I {label} ' + {state}"}}
    steps += [{'tool': 'resize', 'args': {'preset': 'laptop'}}, {'tool': 'navigate', 'args': {'url': inputs}}, {'sleep': 0.5}]
    for label, step in [
        ('hover', {'tool': 'hover', 'args': {'selector': '#h'}}),
        ('menu', {'tool': 'hover', 'args': {'selector': '#m'}}),
        ('click', {'tool': 'click', 'args': {'x': 400, 'y': 240}}),
        ('pointer-drag', {'tool': 'drag', 'args': {'from': {'selector': '#d'}, 'to': {'selector': '#h'}}}),
        ('drop', {'tool': 'drag', 'args': {'from': {'selector': '#card'}, 'to': {'selector': '#b'}}}),
        ('no-drop', {'tool': 'drag', 'args': {'from': {'selector': '#card'}, 'to': {'selector': '#nodrop'}}}),
        ('copy', {'tool': 'drag', 'args': {'from': {'selector': '#card'}, 'to': {'selector': '#copyzone'}}}),
        ('cancel', {'tool': 'drag', 'args': {'from': {'selector': '#cancel'}, 'to': {'selector': '#b'}}}),
        ('slider', {'tool': 'drag', 'args': {'from': {'selector': '#knob'}, 'to': {'selector': '#s'}}}),
        ('scroll', {'tool': 'scroll', 'args': {'direction': 'down', 'amount': 500}}),
    ] + [(f'click-{x}-{y}', {'tool': 'click', 'args': {'x': x, 'y': y}}) for x, y in CLICKS]:
        steps += [step, {'sleep': 0.4 if label == 'scroll' else 0.1}, report(label)]
    json.dump(steps, open(f'{out}/steps.json', 'w'))
    json.dump(shots, open(f'{out}/shots.json', 'w'))

# Clicks anywhere on screen land on exactly that CSS pixel, at whatever scale the page is drawn.
CLICKS = [(1, 1), (37, 41), (513, 277), (777, 333), (1001, 59), (1499, 955), (250, 904)]

# What each input step should leave behind.
EXPECT = {
    **{f'click-{x}-{y}': (lambda x, y: lambda s: f'click {x},{y} ' in s['log'] + ' ')(x, y) for x, y in CLICKS},
    'hover': lambda s: s['h'] == 'rgb(22, 163, 74)' and s['seen'].get('mousemove') == 0,
    'menu': lambda s: s['menu'] == 'block' and s['h'] == 'rgb(221, 221, 221)',
    'click': lambda s: 'click 400,240 h' in s['log'] and s['seen'].get('mousedown') == 1 and s['seen'].get('mouseup') == 0,
    'pointer-drag': lambda s: 'dropped 370,210' in s['log'],
    'drop': lambda s: s['card'] == 'b' and 'drop b card' in s['log'] and 'dragend move' in s['log'],
    'no-drop': lambda s: s['card'] == 'b' and 'dragend none' in s['log'],
    'copy': lambda s: 'copied card' in s['log'] and 'dragend copy' in s['log'],
    'cancel': lambda s: 'cancelled' in s['log'] and 'dragend' not in s['log'],
    'slider': lambda s: 'slider at 140px' in s['log'] and 'gave up' not in s['log'],
    'scroll': lambda s: s['scroll'] == 500 and s['selection'] == '',
}

def report(out, runs):
    reference = json.load(open(f'{out}/reference.json'))
    sizes = json.load(open(f'{os.path.dirname(os.path.abspath(__file__))}/sizes.json'))
    shots = json.load(open(f'{out}/shots.json'))
    imgdiff = f'{out}/imgdiff'
    failures = 0
    def verdict(ok, line):
        nonlocal failures
        failures += not ok
        print(('  ok    ' if ok else '  FAIL  ') + line)
    for run in runs:
        print(f'== {run}')
        log = open(f'{out}/{run}.log').read()
        values = lambda tag: {m.group(1): json.loads(json.loads('"' + m.group(2) + '"')) for m in re.finditer(rf'^"{tag} (\S+) (.*)"$', log, re.M)}
        measured = values('M')
        for name, got in measured.items():
            want = reference['laptop' if name == 'resized-laptop' else name]
            diffs = [k for k in set(want) | set(got) if want.get(k) != got.get(k)]
            verdict(not diffs, f'{name}: what the page measures' + (f' differs in {", ".join(diffs)}' if diffs else ' matches'))
        for name, (w, h) in sizes.items():
            shot = lambda kind: f'{out}/{run}/steps.json.{shots[f"{name}-{kind}"]}.jpg'
            view = float(subprocess.check_output([imgdiff, f'{out}/reference-{name}.png', shot('view')]))
            top = float(subprocess.check_output([imgdiff, f'{out}/reference-{name}.png', shot('full'), '--top']))
            scale = min(w, 1280) / w
            want_height = round(reference[name]['scrollHeight'] * scale)
            got_height = int(re.search(r'pixelHeight: (\d+)', subprocess.check_output(['sips', '-g', 'pixelHeight', shot('full')], text=True)).group(1))
            verdict(view < 1, f'{name}: screenshot {view:.2f}% of pixels off')
            verdict(top < 1 and abs(got_height - want_height) <= 1, f'{name}: full page {got_height} px tall (want {want_height}), first screen {top:.2f}% off')
        s = re.search(r'^"S (.*)"$', log, re.M)
        s = json.loads(json.loads('"' + s.group(1) + '"')) if s else {}
        verdict(s.get('y') == 1500 and s.get('hero') == 982 and s.get('inner') == 982, f'full page from part way down: back at {s.get("y")}, hero {s.get("hero")}')
        verdict('has no <meta name="viewport"' in log, 'a phone-sized page without width=device-width gets the note')
        for label, check in EXPECT.items():
            got = values('I').get(label)
            verdict(bool(got) and check(got), f'input {label}' + ('' if got and check(got) else f': {got}'))
    print('\n' + ('All checks pass.' if not failures else f'{failures} checks failed.'))
    return 1 if failures else 0

if __name__ == '__main__':
    if sys.argv[1] == 'steps': steps(sys.argv[2], sys.argv[3])
    else: sys.exit(report(sys.argv[2], sys.argv[3:]))
