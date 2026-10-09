#!/usr/bin/env python3
"""Copy the Mach-O dependency closure into a Tauri .app and ad-hoc sign it."""
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys


def run(*args):
    return subprocess.check_output(args, text=True).strip()


app = Path(sys.argv[1]).resolve()
binary = app / 'Contents/MacOS/nuzky-app'
frameworks = app / 'Contents/Frameworks'
frameworks.mkdir(parents=True, exist_ok=True)


def dependencies(file):
    return [line.strip().split(' (compatibility version')[0]
            for line in run('otool', '-L', str(file)).splitlines()[1:]]


def system_lib(name):
    return name.startswith(('/System/Library/', '/usr/lib/'))


def resolve(name, owner):
    def expand(value):
        return value.replace('@loader_path', str(owner.parent)).replace(
            '@executable_path', str(binary.parent))

    if name.startswith('@rpath/'):
        commands = run('otool', '-l', str(owner))
        paths = re.findall(r'cmd LC_RPATH\n.*?path (.*?) \(offset', commands, re.S)
        candidates = [Path(expand(p)) / name.removeprefix('@rpath/') for p in paths]
    else:
        candidates = [Path(expand(name))]
    for candidate in candidates:
        if candidate.is_file():
            return candidate.resolve()
    raise RuntimeError(f'Cannot resolve {name} required by {owner}')


# Read original install names before editing; recursive dependencies still point
# into Homebrew here, so resolution must use each original dylib's directory.
copied = {}
pending = [(binary, binary)]
processed = set()
while pending:
    original, dest = pending.pop()
    if original in processed:
        continue
    processed.add(original)
    install_id = None
    if dest.suffix == '.dylib':
        install_id = run('otool', '-D', str(original)).splitlines()[-1]
    for name in dependencies(original):
        if name == install_id or system_lib(name):
            continue
        source = resolve(name, original)
        target = frameworks / source.name
        if target.name in copied and copied[target.name] != source:
            raise RuntimeError(f'Dylib basename collision: {source}, {copied[target.name]}')
        if target.name not in copied:
            copied[target.name] = source
            shutil.copy2(source, target)
            os.chmod(target, 0o755)
            pending.append((source, target))
        prefix = '@executable_path/../Frameworks/' if dest == binary else '@loader_path/'
        run('install_name_tool', '-change', name, prefix + target.name, str(dest))
    if install_id:
        run('install_name_tool', '-id', '@rpath/' + dest.name, str(dest))

for file in [*frameworks.glob('*.dylib'), binary]:
    for dep in dependencies(file):
        if not system_lib(dep) and not dep.startswith(('@executable_path/../Frameworks/', '@loader_path/', '@rpath/' + file.name)):
            raise RuntimeError(f'Unbundled dependency in {file}: {dep}')
    run('codesign', '--force', '--sign', '-', str(file))
manifest = app / 'Contents/Resources/bundled-libraries.txt'
# Write metadata before the final signature so the resource seal remains valid.
manifest.write_text('\n'.join(f'{name}: {source}' for name, source in sorted(copied.items())) + '\n')
run('codesign', '--force', '--sign', '-', str(app))
run('codesign', '--verify', '--deep', '--strict', str(app))
print(f'Bundled and signed {len(copied)} dylibs in {app}')
