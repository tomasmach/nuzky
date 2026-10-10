#!/usr/bin/env python3
"""Runs the end-to-end flows in tests/e2e/ in the real desktop app and keeps the proof: screenshots, logs
and result.json under tmp-test/repro/<flow>/.

    python3 scripts/repro.py --list         flows and what they prove
    python3 scripts/repro.py edit export    run flows; exit code 1 when a check fails
    python3 scripts/repro.py --all

A new flow is a file in tests/e2e/ with a function decorated with @flow; this script finds it.
Each flow gets fresh data, cache and runtime directories, so the app never opens the user's projects or
joins their running Nuzky. On Linux it runs inside headless gamescope with D-Bus switched off, so no window,
dialog or notification reaches the desktop. On macOS the app keeps its window off every screen and never
becomes the active app (src-tauri/src/test_bridge.rs). Media and models come from scripts/fixtures.sh.

Needs Pillow and numpy, and on Linux gamescope, WebKitWebDriver and python-xlib. Vite takes port 1420 (the app's
dev URL) and on Linux WebKitWebDriver 4444. Runs on one machine take turns: a second one waits for the first. A port that something
else holds, such as a dev server, stops the run instead of touching it.
"""
import fcntl, importlib, json, os, shutil, signal, subprocess, sys
from pathlib import Path

sys.dont_write_bytecode = True
TESTS = Path(__file__).resolve().parent.parent / 'tests'
sys.path.insert(0, str(TESTS))
from e2e.harness import FIXTURES, FLOWS, MACOS, MODELS, OUT, ROOT, WEBKIT_DRIVER, port_busy, run_flow, start, started, stop  # noqa: E402

for module in sorted(p.stem for p in (TESTS / 'e2e').glob('*.py') if p.stem != 'harness'):
    importlib.import_module(f'e2e.{module}')


# --- Entry point ------------------------------------------------------------------------------------------

def preflight(names):
    tools = ('ffmpeg', 'npx') if MACOS else ('gamescope', 'ffmpeg', 'npx')
    problems = [f'{tool} is missing' for tool in tools if not shutil.which(tool)]
    problems += [] if MACOS else [f'{path} is missing' for path in (WEBKIT_DRIVER,) if not Path(path).exists()]
    packages = (('PIL', 'Pillow'), ('numpy', 'numpy')) + (() if MACOS else (('Xlib', 'python-xlib'),))
    for module, package in packages:
        try:
            __import__(module)
        except ImportError:
            problems.append(f'Python package {package} is missing')
    if (not (FIXTURES / 'talk.mp4').exists() or ('captions' in names and not (MODELS / 'ggml-small.bin').exists())
            or ('waveform' in names and not (FIXTURES / 'hour.m4a').exists())
            or ('reel' in names and not all((FIXTURES / f).exists() for f in (
                'reel-1.mp4', 'reel-2.mp4', 'reel-3.mp4', 'xdg/data/nuzky/models/ggml-large-v3-turbo-q5_0.bin')))
            or ('first-reel' in names and not all((FIXTURES / f).exists() for f in (
                'first-reel.mov', 'xdg/data/nuzky/models/ggml-large-v3-turbo-q5_0.bin')))):
        problems.append('test media or models are missing: run scripts/fixtures.sh')
    problems += [f'port {port} is in use by another session' for port in ((1420,) if MACOS else (1420, 4444)) if port_busy(port)]
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
    # A stop request (Ctrl+C in a terminal, SIGTERM from a timeout) unwinds through the cleanup below, so Vite,
    # WebKitWebDriver and the app do not keep running and holding their ports.
    signal.signal(signal.SIGTERM, lambda *_: sys.exit(143))
    if os.environ.get('NUZKY_REPRO_INNER') == '1':
        OUT.mkdir(parents=True, exist_ok=True)
        # Checked again here: the port may have been taken while the app was building.
        if port_busy(1420):
            print('port 1420 is in use by another session', file=sys.stderr)
            return 1
        vite = start(['npx', 'vite', '--port', '1420', '--strictPort'], OUT / 'vite.log')
        try:
            if not started(vite, 1420, 30):
                print('Vite did not start, see tmp-test/repro/vite.log', file=sys.stderr)
                return 1
            for name in names:
                run_flow(name)
        finally:
            stop(vite)
        return 0
    # Another session's run holds the ports until it ends; failing on them would waste a whole gate.
    # Kept with the build dependencies, since a terminal running the app may have its own XDG_RUNTIME_DIR.
    locks = Path.home() / '.cache/nuzky/deps'
    locks.mkdir(parents=True, exist_ok=True)
    turn = open(locks / 'repro.lock', 'w')
    try:
        fcntl.flock(turn, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError:
        print('Another UI run uses the app on this machine; this one starts when it ends.', flush=True)
        fcntl.flock(turn, fcntl.LOCK_EX)
    problems = preflight(names)
    if problems:
        print('repro cannot run:\n  ' + '\n  '.join(problems), file=sys.stderr)
        return 1
    if MACOS:
        # The same build scripts/check.sh makes, against Homebrew's FFmpeg as docs/BUILDING.md describes.
        brew = lambda formula: subprocess.run(['brew', '--prefix', formula], capture_output=True, text=True).stdout.strip()
        os.environ.setdefault('PKG_CONFIG_PATH', brew('ffmpeg@8') + '/lib/pkgconfig')
        os.environ.setdefault('SDKROOT', subprocess.run(['xcrun', '--sdk', 'macosx', '--show-sdk-path'], capture_output=True,
                                                        text=True).stdout.strip())
    # nuzky-analyze recognises an exported file again; with the CLI in the same build it gets the GPU too. Every target
    # of the workspace resolves the features `cargo test` does, so this adds only the app's binary to its build.
    subprocess.run(['cargo', 'build', '--locked', '--workspace', '--all-targets'], cwd=ROOT, check=True)
    # The debug app opens ONNX Runtime from the build dependencies when covers run.
    subprocess.run(['node', 'scripts/fetch-onnxruntime.mjs'], cwd=ROOT, check=True, stdout=subprocess.DEVNULL)
    for name in names:
        (OUT / name / 'result.json').unlink(missing_ok=True)
    OUT.mkdir(parents=True, exist_ok=True)
    where, log_name = ('off screen', 'flows.log') if MACOS else ('in headless gamescope', 'gamescope.log')
    print(f'Running {", ".join(names)} {where}; its output goes to tmp-test/repro/{log_name}', flush=True)
    inner = [sys.executable, __file__, *names]
    with open(OUT / log_name, 'w') as log:
        runner = subprocess.Popen(inner if MACOS else ['gamescope', '--backend', 'headless', '-W', '1440', '-H', '900', '--', *inner],
                                  cwd=ROOT, env=dict(os.environ, NUZKY_REPRO_INNER='1'), stdout=log, stderr=subprocess.STDOUT,
                                  start_new_session=True)
        try:
            runner.wait()
        finally:
            if runner.poll() is None:
                stop(runner, timeout=20)
    failed = []
    for name in names:
        result_file = OUT / name / 'result.json'
        result = json.loads(result_file.read_text()) if result_file.exists() else None
        print(f"\n{'passed' if result and result['passed'] else 'FAILED'}  {name}  tmp-test/repro/{name}/")
        for check in result['checks'] if result else []:
            detail = '' if check['ok'] or check['detail'] is None else f": {check['detail']}"
            print(f"  {'ok  ' if check['ok'] else 'FAIL'} {check['check']}{detail}")
        if not result or result['error']:
            print('  ' + (result['error'] if result else f'no result; see tmp-test/repro/{log_name}').strip().replace('\n', '\n  '))
        failed += [] if result and result['passed'] else [name]
    return 1 if failed else 0


if __name__ == '__main__':
    sys.exit(main(sys.argv[1:]))
