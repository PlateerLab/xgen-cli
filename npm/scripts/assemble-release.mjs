import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { chmod, constants, copyFile, lstat, readFile, readdir, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import { distributionMetadata, npmRoot, repoRoot } from './config.mjs';
import { verifyRelease } from './verify-release.mjs';

const supportingFiles = [
  ['install/xgen-installer.sh', 'xgen-installer.sh', 0o755],
  ['install/xgen-installer.ps1', 'xgen-installer.ps1', 0o644],
  ['LICENSE', 'LICENSE.txt', 0o644],
  ['crates/xgen-cli/licenses/NATIVE_RUNTIME_PROVENANCE.md', 'NATIVE_RUNTIME_PROVENANCE.md', 0o644],
  ['THIRD_PARTY_LICENSES.txt', 'THIRD_PARTY_LICENSES.txt', 0o644],
];

async function checkFiles(directory, expected) {
  assert.deepEqual((await readdir(directory)).sort(), [...expected].sort(), 'release asset set differs');
  for (const filename of expected) {
    assert((await lstat(path.join(directory, filename))).isFile(), `${filename} must be a regular file`);
  }
}

export async function assembleRelease(directory, tag) {
  const { catalog } = await distributionMetadata();
  const incoming = catalog.platforms.flatMap(({ asset, tarball }) => [asset, tarball]);
  await checkFiles(directory, incoming);
  for (const specification of catalog.platforms) {
    if (specification.os !== 'win32') await chmod(path.join(directory, specification.asset), 0o755);
  }
  const pack = spawnSync(process.execPath, [path.join(npmRoot, 'scripts/pack.mjs'), '--launcher', '--output-dir', directory], {
    encoding: 'utf8', maxBuffer: 4 * 1024 * 1024,
  });
  assert.equal(pack.status, 0, pack.stderr || pack.error?.message);
  await verifyRelease(directory, tag);
  for (const [source, destination, mode] of supportingFiles) {
    const output = path.join(directory, destination);
    await copyFile(path.join(repoRoot, source), output, constants.COPYFILE_EXCL);
    await chmod(output, mode);
  }
  const outputs = [...incoming, 'xgen-cli.tgz', ...supportingFiles.map(([, destination]) => destination)];
  await checkFiles(directory, outputs);
  const checksums = await Promise.all(outputs.sort().map(async (filename) => {
    const digest = createHash('sha256').update(await readFile(path.join(directory, filename))).digest('hex');
    return `${digest}  ${filename}\n`;
  }));
  await writeFile(path.join(directory, 'checksums.sha256'), checksums.join(''), { flag: 'wx', mode: 0o644 });
  await checkFiles(directory, [...outputs, 'checksums.sha256']);
  console.log(`release assembly: PASS (${outputs.length + 1} assets)`);
}

if (fileURLToPath(import.meta.url) === path.resolve(process.argv[1])) {
  const options = process.argv.slice(2);
  assert(options.length === 4 && options[0] === '--directory' && options[2] === '--tag',
    'usage: assemble-release.mjs --directory DIR --tag vSEMVER');
  await assembleRelease(path.resolve(options[1]), options[3]);
}
