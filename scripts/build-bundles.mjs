#!/usr/bin/env node
import { execFileSync } from 'node:child_process';
import { existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
process.chdir(root);
const target = path.resolve(process.env.CARGO_TARGET_DIR || 'target');
const bundle = path.join(target, 'release', 'bundle');
mkdirSync(bundle, { recursive: true });
const run = (cmd, args) => execFileSync(cmd, args, { stdio: 'inherit' });
const output = (cmd, args) => execFileSync(cmd, args, { encoding: 'utf8' }).trim();
const config = { bundle: {} };
let bundles;

switch (process.platform) {
  case 'linux': {
    bundles = 'appimage,deb';
    const dependencies = ['libwebkit2gtk-4.1-0', 'libgtk-3-0t64', 'libasound2t64'];
    for (const lib of ['libavcodec', 'libavformat', 'libavutil', 'libswscale', 'libswresample']) {
      dependencies.push(lib + output('pkg-config', ['--modversion', lib]).split('.')[0]);
    }
    config.bundle.linux = { deb: { depends: dependencies, section: 'video' } };
    // Allow the bundler's own AppImages to run on hosts without mounted FUSE.
    process.env.APPIMAGE_EXTRACT_AND_RUN = '1';
    break;
  }
  case 'darwin':
    if (process.arch !== 'arm64') throw new Error('Use an Apple Silicon host.');
    bundles = 'app';
    break;
  case 'win32': {
    bundles = 'nsis';
    const ffmpeg = process.env.FFMPEG_DIR;
    if (!ffmpeg) throw new Error('Run scripts/setup-windows-deps.ps1 in this terminal first.');
    const dlls = readdirSync(path.join(ffmpeg, 'bin')).filter(name => name.endsWith('.dll'));
    if (!dlls.some(name => name.startsWith('avcodec-'))) throw new Error('FFmpeg DLLs missing.');
    config.bundle.resources = Object.fromEntries(dlls.map(name => [path.join(ffmpeg, 'bin', name), name]));
    for (const name of ['LICENSE.txt', 'LICENSE', 'README.txt']) {
      if (existsSync(path.join(ffmpeg, name))) config.bundle.resources[path.join(ffmpeg, name)] = `licenses/ffmpeg/${name}`;
    }
    break;
  }
  default: throw new Error(`Unsupported platform: ${process.platform}`);
}
const configPath = path.join(target, 'bundle-config.json');
writeFileSync(configPath, JSON.stringify(config, null, 2));
// Call the installed CLI with Node to avoid Windows .cmd quoting/shell differences.
run(process.execPath, ['node_modules/@tauri-apps/cli/tauri.js', 'build', '--bundles', bundles, '--config', configPath, '--', '--locked']);

if (process.platform === 'darwin') {
  const app = path.join(bundle, 'macos', 'CapOpen.app');
  run('python3', ['scripts/bundle-macos-libs.py', app]);
  const { version } = JSON.parse(readFileSync('src-tauri/tauri.conf.json', 'utf8'));
  mkdirSync(path.join(bundle, 'dmg'), { recursive: true });
  const stage = mkdtempSync(path.join(target, 'dmg-stage-'));
  try {
    run('ditto', [app, path.join(stage, 'CapOpen.app')]);
    symlinkSync('/Applications', path.join(stage, 'Applications'));
    run('hdiutil', ['create', '-ov', '-format', 'UDZO', '-volname', 'CapOpen', '-srcfolder', stage,
      path.join(bundle, 'dmg', `CapOpen_${version}_aarch64.dmg`)]);
  } finally {
    rmSync(stage, { recursive: true, force: true });
  }
}
const info = [
  `Platform: ${process.platform} ${process.arch}`,
  output('rustc', ['--version']),
  output('ffmpeg', ['-version']),
  'FFmpeg redistribution/source checklist: docs/BUILDING.md',
];
// Formulae only: the bundled libraries come from them, and a cask from an untrusted tap makes plain `brew list` fail.
if (process.platform === 'darwin') info.push(output('brew', ['list', '--formula', '--versions']));
if (process.platform === 'linux') info.push(output('dpkg-query', ['-W']));
writeFileSync(path.join(bundle, `build-info-${process.platform}.txt`), info.join('\n') + '\n');
