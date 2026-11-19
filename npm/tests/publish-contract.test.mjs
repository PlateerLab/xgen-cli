import assert from 'node:assert/strict';
import test from 'node:test';

import {
  publicationOrder,
  shouldPromotePreview,
  validateExistingPublication,
} from '../scripts/publish.mjs';

const provenance = {
  provenance: { predicateType: 'https://slsa.dev/provenance/v1' },
};

test('platform packages are ordered before the launcher', () => {
  const reports = [
    { name: '@xgen-ai/cli', integrity: 'sha512-launcher' },
    { name: '@xgen-ai/cli-linux-x64-musl', integrity: 'sha512-linux' },
    { name: '@xgen-ai/cli-darwin-arm64', integrity: 'sha512-macos' },
  ];
  assert.deepEqual(
    publicationOrder(reports, '@xgen-ai/cli').map(({ name }) => name),
    ['@xgen-ai/cli-linux-x64-musl', '@xgen-ai/cli-darwin-arm64', '@xgen-ai/cli'],
  );
});

test('latest follows verified previews only until a stable or newer release exists', () => {
  for (const [older, candidate] of [
    ['0.0.0-bootstrap.0', '0.1.0-rc.7'],
    ['2.0.0-beta.2', '2.0.0-beta.3'],
  ]) {
    const metadata = { versions: [older, candidate], 'dist-tags': { latest: older, next: candidate } };
    assert.equal(shouldPromotePreview(metadata, candidate), true);
    assert.equal(shouldPromotePreview({ ...metadata, 'dist-tags': { latest: candidate, next: candidate } }, candidate), true);
    for (const stable of ['0.0.1', '1.0.0+build-with-hyphen']) {
      assert.equal(shouldPromotePreview({ ...metadata, versions: [...metadata.versions, stable] }, candidate), false);
    }
    assert.equal(shouldPromotePreview({ ...metadata, versions: [...metadata.versions, '3.0.0-rc.1'] }, candidate), false);
  }
});

test('preview promotion fails closed on incomplete or inconsistent registry metadata', () => {
  const version = '1.0.0-rc.1';
  const metadata = { versions: [version], 'dist-tags': { next: version } };
  for (const invalid of [
    {},
    { ...metadata, versions: [] },
    { ...metadata, versions: [version, 'invalid'] },
    { ...metadata, 'dist-tags': { next: '0.9.0-rc.1' } },
    { ...metadata, 'dist-tags': { next: version, latest: '0.9.0' } },
  ]) assert.throws(() => shouldPromotePreview(invalid, version));
  assert.throws(() => shouldPromotePreview(metadata, '1.0.0'));
});

test('publication order requires exactly one launcher', () => {
  assert.throws(
    () => publicationOrder([{ name: '@xgen-ai/cli-linux-x64-musl' }], '@xgen-ai/cli'),
    /exactly one launcher/,
  );
});

test('an identical retry requires integrity, dist-tag, and provenance', () => {
  const report = { name: '@xgen-ai/cli', integrity: 'sha512-exact' };
  const remote = {
    'dist.integrity': 'sha512-exact',
    'dist-tags': { next: '0.1.0-rc.3' },
    'dist.attestations': provenance,
  };
  assert.doesNotThrow(() =>
    validateExistingPublication(remote, report, '0.1.0-rc.3', 'next'),
  );
  assert.throws(
    () =>
      validateExistingPublication(
        { ...remote, 'dist.integrity': 'sha512-different' },
        report,
        '0.1.0-rc.3',
        'next',
      ),
    /already differs/,
  );
  assert.throws(
    () =>
      validateExistingPublication(
        { ...remote, 'dist.attestations': undefined },
        report,
        '0.1.0-rc.3',
        'next',
      ),
    /missing npm provenance/,
  );
});
