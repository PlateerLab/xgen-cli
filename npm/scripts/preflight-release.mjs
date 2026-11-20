// Exercise the production assembly entrypoint with local byte fixtures, never native binaries.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { copyFile, lstat, mkdir, mkdtemp, readFile, readdir, rm, writeFile } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';

import { launcherRoot, npmRoot, repoRoot } from './config.mjs';
import './check.mjs';

const started = performance.now();
const root = await mkdtemp(path.join(os.tmpdir(), 'xgen-release-preflight-'));

async function copy(source, destination) {
  assert((await lstat(source)).isFile(), 'preflight sources must be regular files');
  await mkdir(path.dirname(destination), { recursive: true });
  await copyFile(source, destination);
}

function run(snapshot, relative, args, env) {
  const result = spawnSync(process.execPath, [path.join(snapshot, relative), ...args], {
    cwd: snapshot, env, encoding: 'utf8', timeout: 60_000, maxBuffer: 4 * 1024 * 1024,
  });
  assert(!result.error, result.error?.message);
  return result;
}

try {
  for (const version of ['0.0.0-preflight.0', '0.0.0']) {
    const snapshot = path.join(root, version);
    await mkdir(snapshot);
    for (const filename of await readdir(path.join(npmRoot, 'scripts'))) {
      if (filename.endsWith('.mjs')) await copy(path.join(npmRoot, 'scripts', filename), path.join(snapshot, 'npm/scripts', filename));
    }
    for (const filename of [
      'LICENSE', 'THIRD_PARTY_LICENSES.txt', 'install/xgen-installer.sh', 'install/xgen-installer.ps1',
      'crates/xgen-cli/licenses/NATIVE_RUNTIME_PROVENANCE.md', 'npm/platforms.json',
    ]) await copy(path.join(repoRoot, filename), path.join(snapshot, filename));
    const launcher = JSON.parse(await readFile(path.join(launcherRoot, 'package.json'), 'utf8'));
    for (const filename of launcher.files) {
      const source = filename === 'LICENSE' ? path.join(repoRoot, filename) : path.join(launcherRoot, filename);
      await copy(source, path.join(snapshot, 'npm/packages/cli', filename));
    }
    launcher.version = version;
    launcher.optionalDependencies = Object.fromEntries(Object.keys(launcher.optionalDependencies).map((name) => [name, version]));
    await writeFile(path.join(snapshot, 'npm/packages/cli/package.json'), JSON.stringify(launcher));
    const workspace = JSON.parse(await readFile(path.join(npmRoot, 'package.json'), 'utf8'));
    workspace.version = version;
    const lock = JSON.parse(await readFile(path.join(npmRoot, 'package-lock.json'), 'utf8'));
    lock.version = version;
    lock.packages[''].version = version;
    await writeFile(path.join(snapshot, 'npm/package.json'), JSON.stringify(workspace));
    await writeFile(path.join(snapshot, 'npm/package-lock.json'), JSON.stringify(lock));
    await writeFile(path.join(snapshot, 'Cargo.toml'), `[workspace.package]\nversion = "${version}"\n`);
    const fixtureHome = path.join(snapshot, 'home');
    await mkdir(fixtureHome, { mode: 0o700 });
    const npmrc = path.join(fixtureHome, '.npmrc');
    await writeFile(npmrc, '');
    await writeFile(path.join(fixtureHome, 'global.npmrc'), '');
    const allowed = new Set(['PATH', 'LANG', 'LC_ALL', 'SYSTEMROOT', 'WINDIR', 'COMSPEC', 'PATHEXT', 'TMP', 'TEMP', 'TMPDIR', 'NPM_EXECPATH']);
    const env = { ...Object.fromEntries(Object.entries(process.env).filter(([name]) => allowed.has(name.toUpperCase()))), HOME: fixtureHome, USERPROFILE: fixtureHome,
      npm_config_userconfig: npmrc, npm_config_globalconfig: path.join(fixtureHome, 'global.npmrc'), npm_config_cache: path.join(fixtureHome, 'cache'),
      npm_config_offline: 'true', npm_config_audit: 'false', npm_config_fund: 'false',
      npm_config_ignore_scripts: 'true', npm_config_update_notifier: 'false',
    };
    assert.equal((await readdir(path.join(snapshot, 'npm'))).includes('node_modules'), false);
    const catalog = JSON.parse(await readFile(path.join(snapshot, 'npm/platforms.json'), 'utf8'));
    const inputs = path.join(snapshot, 'inputs');
    await mkdir(inputs);
    for (const [index, specification] of catalog.platforms.entries()) {
      // Distinct binary-like payloads test byte parity. Nothing here can run as a native program.
      await writeFile(path.join(inputs, specification.asset), Buffer.from(`ASSEMBLY_FIXTURE_ONLY\0${version}\0${index}\n`));
      const packed = run(snapshot, 'npm/scripts/pack.mjs', ['--target', specification.target,
        '--binary', path.join(inputs, specification.asset), '--output-dir', inputs], env);
      assert.equal(packed.status, 0, packed.stderr);
    }
    const inputNames = await readdir(inputs);
    async function candidate(name) {
      const directory = path.join(snapshot, name);
      await mkdir(directory);
      for (const filename of inputNames) await copy(path.join(inputs, filename), path.join(directory, filename));
      return directory;
    }
    const directory = await candidate('bundle');
    const assembled = run(snapshot, 'npm/scripts/assemble-release.mjs', ['--directory', directory, '--tag', `v${version}`], env);
    assert.equal(assembled.status, 0, assembled.stderr);
    const entries = (await readFile(path.join(directory, 'checksums.sha256'), 'utf8')).trim().split('\n');
    const names = [];
    for (const line of entries) {
      const [, digest, filename] = /^([0-9a-f]{64})  (.+)$/.exec(line);
      assert.equal(digest, createHash('sha256').update(await readFile(path.join(directory, filename))).digest('hex'));
      names.push(filename);
    }
    assert.equal(new Set(names).size, names.length, 'checksum entries must be unique');
    assert.deepEqual(names.sort(), (await readdir(directory)).filter((filename) => filename !== 'checksums.sha256').sort());
    for (const fault of ['extra', 'mismatch', 'missing']) {
      const damaged = await candidate(fault);
      if (fault === 'extra') await writeFile(path.join(damaged, 'unexpected.txt'), 'fixture');
      if (fault === 'mismatch') await writeFile(path.join(damaged, catalog.platforms[0].asset), 'different fixture');
      if (fault === 'missing') await rm(path.join(damaged, catalog.platforms.at(-1).tarball));
      const rejected = run(snapshot, 'npm/scripts/assemble-release.mjs', ['--directory', damaged, '--tag', `v${version}`], env);
      assert.notEqual(rejected.status, 0);
      assert(rejected.stderr.includes(fault === 'mismatch' ? 'must contain the exact GitHub release binary' : 'release asset set differs'), rejected.stderr);
    }
  }
  console.log(`release preflight: PASS (stable/prerelease assembly, checksum coverage, 6 rejected input sets; ${((performance.now() - started) / 1000).toFixed(1)}s; fixture bytes only)`);
} finally {
  // Every file in this private directory is created by this preflight; no user configuration is copied.
  await rm(root, { recursive: true, force: true });
}
