#!/usr/bin/env node
import { execFileSync } from 'node:child_process';
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { VERSION as ORT_VERSION, fetchOnnxRuntime } from './fetch-onnxruntime.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
process.chdir(root);
const target = path.resolve(process.env.CARGO_TARGET_DIR || 'target');
const bundle = path.join(target, 'release', 'bundle');
mkdirSync(bundle, { recursive: true });
const run = (cmd, args) => execFileSync(cmd, args, { stdio: 'inherit' });
const output = (cmd, args) => execFileSync(cmd, args, { encoding: 'utf8' }).trim();
const config = { bundle: {} };
let bundles;
// ONNX Runtime ships beside the app and is opened only when covers run (crates/vision/src/runtime.rs).
const ort = await fetchOnnxRuntime();
const ortNotices = Object.fromEntries(ort.notices.map((n) => [n, `licenses/onnxruntime/${path.basename(n)}`]));

switch (process.platform) {
  case 'linux': {
    bundles = 'appimage,deb';
    const dependencies = ['libwebkit2gtk-4.1-0', 'libgtk-3-0t64', 'libasound2t64'];
    for (const lib of ['libavcodec', 'libavformat', 'libavutil', 'libswscale', 'libswresample']) {
      dependencies.push(lib + output('pkg-config', ['--modversion', lib]).split('.')[0]);
    }
    config.bundle.linux = { deb: { depends: dependencies, section: 'video' } };
    // Resources land in usr/lib/Nuzky in both the AppImage and the deb, where the app looks.
    config.bundle.resources = { [ort.library]: path.basename(ort.library), ...ortNotices };
    // Allow the bundler's own AppImages to run on hosts without mounted FUSE.
    process.env.APPIMAGE_EXTRACT_AND_RUN = '1';
    break;
  }
  case 'darwin':
    if (process.arch !== 'arm64') throw new Error('Use an Apple Silicon host.');
    bundles = 'app';
    // The library itself goes to Contents/Frameworks through bundle-macos-libs.py below.
    config.bundle.resources = ortNotices;
    break;
  case 'win32': {
    bundles = 'nsis';
    const ffmpeg = process.env.FFMPEG_DIR;
    if (!ffmpeg) throw new Error('Run scripts/setup-windows-deps.ps1 in this terminal first.');
    const dlls = readdirSync(path.join(ffmpeg, 'bin')).filter(name => name.endsWith('.dll'));
    if (!dlls.some(name => name.startsWith('avcodec-'))) throw new Error('FFmpeg DLLs missing.');
    config.bundle.resources = Object.fromEntries(dlls.map(name => [path.join(ffmpeg, 'bin', name), name]));
    Object.assign(config.bundle.resources, { [ort.library]: path.basename(ort.library) }, ortNotices);
    for (const name of ['LICENSE.txt', 'LICENSE', 'README.txt']) {
      if (existsSync(path.join(ffmpeg, name))) config.bundle.resources[path.join(ffmpeg, name)] = `licenses/ffmpeg/${name}`;
    }
    // Tauri drops the drive letter from an absolute resource path, and GitHub's runner keeps these files on C:
    // and the checkout on D:. Copied beside the build, they are named relative to src-tauri instead.
    const stage = path.join(target, 'bundle-resources');
    rmSync(stage, { recursive: true, force: true });
    config.bundle.resources = Object.fromEntries(Object.entries(config.bundle.resources).map(([from, to]) => {
      const copy = path.join(stage, to);
      mkdirSync(path.dirname(copy), { recursive: true });
      copyFileSync(from, copy);
      return [path.relative('src-tauri', copy), to];
    }));
    break;
  }
  default: throw new Error(`Unsupported platform: ${process.platform}`);
}
const configPath = path.join(target, 'bundle-config.json');
writeFileSync(configPath, JSON.stringify(config, null, 2));
// Call the installed CLI with Node to avoid Windows .cmd quoting/shell differences.
run(process.execPath, ['node_modules/@tauri-apps/cli/tauri.js', 'build', '--bundles', bundles, '--config', configPath, '--', '--locked']);

if (process.platform === 'darwin') {
  const app = path.join(bundle, 'macos', 'Nuzky.app');
  run('python3', ['scripts/bundle-macos-libs.py', app, ort.library]);
  const { version } = JSON.parse(readFileSync('src-tauri/tauri.conf.json', 'utf8'));
  mkdirSync(path.join(bundle, 'dmg'), { recursive: true });
  const stage = mkdtempSync(path.join(target, 'dmg-stage-'));
  try {
    run('ditto', [app, path.join(stage, 'Nuzky.app')]);
    symlinkSync('/Applications', path.join(stage, 'Applications'));
    run('hdiutil', ['create', '-ov', '-format', 'UDZO', '-volname', 'Nuzky', '-srcfolder', stage,
      path.join(bundle, 'dmg', `Nuzky_${version}_aarch64.dmg`)]);
  } finally {
    rmSync(stage, { recursive: true, force: true });
  }
}
const info = [
  `Platform: ${process.platform} ${process.arch}`,
  output('rustc', ['--version']),
  output('ffmpeg', ['-version']),
  `ONNX Runtime ${ORT_VERSION} (scripts/fetch-onnxruntime.mjs pins the official release archive)`,
  'FFmpeg redistribution/source checklist: docs/BUILDING.md',
];
// Formulae only: the bundled libraries come from them, and a cask from an untrusted tap makes plain `brew list` fail.
if (process.platform === 'darwin') info.push(output('brew', ['list', '--formula', '--versions']));
if (process.platform === 'linux') info.push(output('dpkg-query', ['-W']));
writeFileSync(path.join(bundle, `build-info-${process.platform}.txt`), info.join('\n') + '\n');
