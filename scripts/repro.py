#!/usr/bin/env python3
"""Runs the end-to-end flows in tests/e2e/ in the real desktop app and keeps the proof: screenshots, logs
and result.json under tmp-test/repro/<flow>/.

    python3 scripts/repro.py --list         flows and what they prove
    python3 scripts/repro.py edit export    run flows; exit code 1 when a check fails
    python3 scripts/repro.py --all

A new flow is a file in tests/e2e/ with a function decorated with @flow; this script finds it.
Each flow gets fresh data, cache and runtime directories, so the app never opens the user's projects or
joins their running CapOpen. It runs inside headless gamescope with D-Bus switched off, so no window,
dialog or notification reaches the desktop. Media and models come from scripts/fixtures.sh.

Needs gamescope, WebKitWebDriver, Pillow and python-xlib. Vite takes port 1420 (the app's dev URL) and
WebKitWebDriver 4444. A busy port belongs to another session, so the run stops instead of touching it.
"""
import importlib, json, os, shutil, subprocess, sys
from pathlib import Path

sys.dont_write_bytecode = True
TESTS = Path(__file__).resolve().parent.parent / 'tests'
sys.path.insert(0, str(TESTS))
from e2e.harness import FIXTURES, FLOWS, MODELS, OUT, ROOT, WEBKIT_DRIVER, port_busy, run_flow, start, stop, wait  # noqa: E402

for module in sorted(p.stem for p in (TESTS / 'e2e').glob('*.py') if p.stem != 'harness'):
    importlib.import_module(f'e2e.{module}')


# --- Entry point ------------------------------------------------------------------------------------------

def preflight(names):
    problems = [f'{tool} is missing' for tool in ('gamescope', 'ffmpeg', 'npx') if not shutil.which(tool)]
    problems += [f'{path} is missing' for path in (WEBKIT_DRIVER,) if not Path(path).exists()]
    for module, package in (('PIL', 'Pillow'), ('Xlib', 'python-xlib')):
        try:
            __import__(module)
        except ImportError:
            problems.append(f'Python package {package} is missing')
    if not (FIXTURES / 'talk.mp4').exists() or ('captions' in names and not (MODELS / 'ggml-small.bin').exists()):
        problems.append('test media or models are missing: run scripts/fixtures.sh')
    problems += [f'port {port} is in use by another session' for port in (1420, 4444) if port_busy(port)]
    return problems


def main(args):
    if not args or args == ['--list']:
        print('Flows (python3 scripts/repro.py <flow>... | --all):\n')
        for name, (description, _, _) in FLOWS.items():
            print(f'  {name:<12} {description}')
        return 0
    names = list(FLOWS) if args == ['--all'] else args
    unknown = [n for n in names if n not in FLOWS]
    if unknown:
        print(f'Unknown flow: {", ".join(unknown)}. Run python3 scripts/repro.py --list.', file=sys.stderr)
        return 2
    if os.environ.get('CAPOPEN_REPRO_INNER') == '1':
        OUT.mkdir(parents=True, exist_ok=True)
        vite = start(['npx', 'vite', '--port', '1420', '--strictPort'], OUT / 'vite.log')
        try:
            if not wait(lambda: port_busy(1420), 30):
                print('Vite did not start, see tmp-test/repro/vite.log', file=sys.stderr)
                return 1
            for name in names:
                run_flow(name)
        finally:
            stop(vite)
        return 0
    problems = preflight(names)
    if problems:
        print('repro cannot run:\n  ' + '\n  '.join(problems), file=sys.stderr)
        return 1
    subprocess.run(['cargo', 'build', '--locked', '-p', 'capopen-app', '-p', 'capopen-cli'], cwd=ROOT, check=True)
    for name in names:
        (OUT / name / 'result.json').unlink(missing_ok=True)
    OUT.mkdir(parents=True, exist_ok=True)
    print(f'Running {", ".join(names)} in headless gamescope; its output goes to tmp-test/repro/gamescope.log', flush=True)
    with open(OUT / 'gamescope.log', 'w') as log:
        subprocess.run(['gamescope', '--backend', 'headless', '-W', '1440', '-H', '900', '--', sys.executable, __file__, *names],
                       cwd=ROOT, env=dict(os.environ, CAPOPEN_REPRO_INNER='1'), stdout=log, stderr=subprocess.STDOUT)
    failed = []
    for name in names:
        result_file = OUT / name / 'result.json'
        result = json.loads(result_file.read_text()) if result_file.exists() else None
        print(f"\n{'passed' if result and result['passed'] else 'FAILED'}  {name}  tmp-test/repro/{name}/")
        for check in result['checks'] if result else []:
            detail = '' if check['ok'] or check['detail'] is None else f": {check['detail']}"
            print(f"  {'ok  ' if check['ok'] else 'FAIL'} {check['check']}{detail}")
        if not result or result['error']:
            print('  ' + (result['error'] if result else 'no result; see tmp-test/repro/gamescope.log').strip().replace('\n', '\n  '))
        failed += [] if result and result['passed'] else [name]
    return 1 if failed else 0


if __name__ == '__main__':
    sys.exit(main(sys.argv[1:]))
