import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { copyFile, mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';

import { npmRoot } from '../scripts/config.mjs';

test('assembly verifier loads from a fresh checkout without npm dependencies', async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), 'xgen-assembly-contract-'));
  try {
    const scripts = path.join(root, 'npm', 'scripts');
    await mkdir(scripts, { recursive: true });
    for (const filename of ['config.mjs', 'verify-release.mjs']) {
      await copyFile(path.join(npmRoot, 'scripts', filename), path.join(scripts, filename));
    }
    const runner = path.join(root, 'probe.mjs');
    await writeFile(runner, `
      import assert from 'node:assert/strict';
      import { normalizeDryRunReport } from './npm/scripts/verify-release.mjs';
      assert.equal(normalizeDryRunReport({ id: 'fixture@1.0.0' }).id, 'fixture@1.0.0');
    `);
    const env = { ...process.env };
    delete env.NODE_PATH;
    delete env.NODE_OPTIONS;
    const result = spawnSync(process.execPath, [runner], { env, encoding: 'utf8' });
    assert.equal(result.status, 0, result.stderr);
  } finally {
    // This directory is created above and contains only the copied public sources and probe.
    await rm(root, { recursive: true, force: true });
  }
});
