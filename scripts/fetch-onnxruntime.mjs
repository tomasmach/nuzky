#!/usr/bin/env node
// Fetches Microsoft's official ONNX Runtime build, which crates/vision loads at run time instead of
// linking it, into the build dependencies directory: the release archive is checked against its pinned
// SHA-256 and only the library and its licence notices are kept. Prints the library's path. Debug
// builds find it there; scripts/build-bundles.mjs puts it in the installers.
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, renameSync, rmSync, writeFileSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

// Keep in step with RUNTIME_VERSION in crates/vision/src/runtime.rs.
export const VERSION = '1.28.3';
// github.com/microsoft/onnxruntime release v1.28.3 (commit 0d68ff6b3b72b04aac578decd6c4c45d322bb962), MIT.
const PINS = {
  'linux-x64': {
    asset: `onnxruntime-linux-x64-${VERSION}.tgz`,
    sha256: 'db14e4863bd37893fc59729d986ab2a0d043d10b7d44da1913c4982b7e3d009c',
    library: [`lib/libonnxruntime.so.${VERSION}`, 'libonnxruntime.so.1'],
  },
  'darwin-arm64': {
    asset: `onnxruntime-osx-arm64-${VERSION}.tgz`,
    sha256: 'c436bd9f47dbce6f6311ccc21de97f829b3c862c09d24aa321958cb72f0a1d32',
    library: [`lib/libonnxruntime.${VERSION}.dylib`, 'libonnxruntime.1.dylib'],
  },
  'win32-x64': {
    asset: `onnxruntime-win-x64-${VERSION}.zip`,
    sha256: '1d6fab48e85f948436af7c8c971d2c145cf224e2c444755dec894f8b0de11a83',
    library: ['lib/onnxruntime.dll', 'onnxruntime.dll'],
  },
};
const NOTICES = ['LICENSE', 'ThirdPartyNotices.txt'];

/** The same directory the setup scripts keep build dependencies in. */
export function depsDir() {
  if (process.env.NUZKY_DEPS) return process.env.NUZKY_DEPS;
  if (process.platform === 'win32') return path.join(process.env.LOCALAPPDATA, 'Nuzky', 'build-deps');
  return path.join(os.homedir(), '.cache', 'nuzky', 'deps');
}

const sha256 = (file) => createHash('sha256').update(readFileSync(file)).digest('hex');

/** Returns `{dir, library, notices}`; downloads only when the checked copy is missing. */
export async function fetchOnnxRuntime() {
  const pin = PINS[`${process.platform}-${process.arch}`];
  if (!pin) throw new Error(`No ONNX Runtime build is pinned for ${process.platform}-${process.arch}.`);
  const deps = depsDir();
  const dir = path.join(deps, `onnxruntime-${VERSION}`);
  const result = { dir, library: path.join(dir, pin.library[1]), notices: NOTICES.map((n) => path.join(dir, n)) };
  if (existsSync(path.join(dir, '.ready'))) return result;
  mkdirSync(deps, { recursive: true });
  const archive = path.join(deps, pin.asset);
  if (!existsSync(archive) || sha256(archive) !== pin.sha256) {
    const url = `https://github.com/microsoft/onnxruntime/releases/download/v${VERSION}/${pin.asset}`;
    console.error(`> ${url}`);
    const response = await fetch(url);
    if (!response.ok) throw new Error(`Downloading ${url}: HTTP ${response.status}`);
    writeFileSync(`${archive}.part`, Buffer.from(await response.arrayBuffer()));
    if (sha256(`${archive}.part`) !== pin.sha256) throw new Error(`${archive}.part does not match its SHA-256`);
    renameSync(`${archive}.part`, archive);
  }
  const stage = mkdtempSync(path.join(deps, 'onnxruntime-stage-'));
  try {
    const root = pin.asset.replace(/\.(tgz|zip)$/, '');
    // Windows' own bsdtar reads zip archives; Git's GNU tar on PATH does not.
    const tar = process.platform === 'win32' ? path.join(process.env.SystemRoot, 'System32', 'tar.exe') : 'tar';
    const members = [pin.library[0], ...NOTICES].map((m) => `${root}/${m}`);
    execFileSync(tar, ['-xf', archive, '-C', stage, ...members], { stdio: 'inherit' });
    const out = path.join(stage, 'out');
    mkdirSync(out);
    copyFileSync(path.join(stage, root, pin.library[0]), path.join(out, pin.library[1]));
    for (const notice of NOTICES) copyFileSync(path.join(stage, root, notice), path.join(out, notice));
    writeFileSync(path.join(out, '.ready'), `${pin.asset} ${pin.sha256}\n`);
    rmSync(dir, { recursive: true, force: true });
    renameSync(out, dir);
  } finally {
    rmSync(stage, { recursive: true, force: true });
  }
  return result;
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  console.log((await fetchOnnxRuntime()).library);
}
