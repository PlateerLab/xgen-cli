import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { readFile } from 'node:fs/promises';
import test from 'node:test';

const require = createRequire(import.meta.url);
const catalog = JSON.parse(
  await readFile(new URL('../platforms.json', import.meta.url), 'utf8'),
);
const { checkNodeVersion, resolvePlatformBinary, selectPlatform } = require(
  '../packages/cli/lib/platform.cjs',
);
const launcher = JSON.parse(
  await readFile(new URL('../packages/cli/package.json', import.meta.url), 'utf8'),
);

const expected = [
  ['linux', 'x64', '@xgen-ai/cli-linux-x64-musl', 'bin/xgen'],
  ['linux', 'arm64', '@xgen-ai/cli-linux-arm64-musl', 'bin/xgen'],
  ['darwin', 'x64', '@xgen-ai/cli-darwin-x64', 'bin/xgen'],
  ['darwin', 'arm64', '@xgen-ai/cli-darwin-arm64', 'bin/xgen'],
  ['win32', 'x64', '@xgen-ai/cli-win32-x64', 'bin/xgen.exe'],
];

test('platform catalog selects every supported native package exactly', () => {
  for (const [platform, architecture, packageName, binaryPath] of expected) {
    const selected = selectPlatform(catalog, platform, architecture);
    assert.equal(selected.packageName, packageName);
    assert.equal(selected.binaryPath, binaryPath);
  }
});

test('unsupported platform fails without a fallback download', () => {
  assert.throws(
    () => selectPlatform(catalog, 'freebsd', 'x64'),
    (error) =>
      error.code === 'XGEN_UNSUPPORTED_PLATFORM' &&
      error.message === 'XGEN does not provide a native binary for freebsd/x64',
  );
});

test('native package resolution uses the exact package subpath', () => {
  const selected = selectPlatform(catalog, 'linux', 'x64');
  const requests = [];
  const resolved = resolvePlatformBinary(selected, (request) => {
    requests.push(request);
    return '/verified/native/xgen';
  });
  assert.equal(resolved, '/verified/native/xgen');
  assert.deepEqual(requests, ['@xgen-ai/cli-linux-x64-musl/bin/xgen']);
});

test('missing optional dependency produces a bounded recovery message', () => {
  const selected = selectPlatform(catalog, 'linux', 'x64');
  assert.throws(
    () =>
      resolvePlatformBinary(selected, () => {
        const error = new Error('host-specific resolution detail');
        error.code = 'MODULE_NOT_FOUND';
        throw error;
      }),
    (error) =>
      error.code === 'XGEN_PLATFORM_PACKAGE_MISSING' &&
      error.message ===
        'The required XGEN native package @xgen-ai/cli-linux-x64-musl is not installed. ' +
          'Reinstall @xgen-ai/cli without --omit=optional.' &&
      !error.message.includes('host-specific'),
  );
});

test('launcher accepts Node.js versions at or above the declared engine', () => {
  const engine = launcher.engines.node;
  for (const version of ['v22.14.0', 'v22.14.1', 'v22.22.1', 'v23.0.0', 'v24.1.0']) {
    assert.doesNotThrow(() => checkNodeVersion(engine, version), version);
  }
  assert.doesNotThrow(() => checkNodeVersion(engine, process.version));
});

test('older Node.js is reported before the missing native package', () => {
  const engine = launcher.engines.node;
  for (const version of ['v22.13.1', 'v22.0.0', 'v20.18.0', 'v18.20.4', 'not-a-version']) {
    assert.throws(
      () => checkNodeVersion(engine, version),
      (error) =>
        error.code === 'XGEN_UNSUPPORTED_NODE' &&
        error.message ===
          `XGEN requires Node.js 22.14.0 or later (current: ${version}). ` +
            'Upgrade Node.js, then reinstall @xgen-ai/cli.',
      version,
    );
  }
});

test('launcher rejects an engine range it cannot evaluate', () => {
  for (const engine of ['^22.14.0', '>=22.14', '>=22.14.0 <25']) {
    assert.throws(
      () => checkNodeVersion(engine, 'v22.14.0'),
      /XGEN npm launcher engine range is invalid/,
      engine,
    );
  }
});
