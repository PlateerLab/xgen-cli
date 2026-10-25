'use strict';

// Optional platform packages declare the same engines range, so npm skips them
// silently on an older Node.js. Check first so that case is not reported as
// a missing optional dependency.
function checkNodeVersion(engine, version) {
  const required = /^>=(\d+)\.(\d+)\.(\d+)$/.exec(engine);
  if (!required) {
    throw new Error('XGEN npm launcher engine range is invalid');
  }
  const current = /^v?(\d+)\.(\d+)\.(\d+)/.exec(version);
  if (current) {
    const difference = [1, 2, 3]
      .map((index) => Number(current[index]) - Number(required[index]))
      .find((value) => value !== 0);
    if (difference === undefined || difference > 0) {
      return;
    }
  }
  const minimum = engine.slice(2);
  const error = new Error(
    `XGEN requires Node.js ${minimum} or later (current: ${version}). ` +
      'Upgrade Node.js, then reinstall @xgen-ai/cli.',
  );
  error.code = 'XGEN_UNSUPPORTED_NODE';
  throw error;
}

function selectPlatform(catalog, platform, architecture) {
  if (!catalog || catalog.schemaVersion !== 1 || !Array.isArray(catalog.platforms)) {
    throw new Error('XGEN npm platform catalog is invalid');
  }

  const matches = catalog.platforms.filter(
    (candidate) => candidate.os === platform && candidate.cpu === architecture,
  );
  if (matches.length !== 1) {
    const error = new Error(`XGEN does not provide a native binary for ${platform}/${architecture}`);
    error.code = 'XGEN_UNSUPPORTED_PLATFORM';
    throw error;
  }
  return matches[0];
}

function resolvePlatformBinary(specification, resolver = require.resolve) {
  const request = `${specification.packageName}/${specification.binaryPath}`;
  try {
    return resolver(request);
  } catch (cause) {
    const error = new Error(
      `The required XGEN native package ${specification.packageName} is not installed. ` +
        'Reinstall @xgen-ai/cli without --omit=optional.',
    );
    error.code = 'XGEN_PLATFORM_PACKAGE_MISSING';
    error.cause = cause;
    throw error;
  }
}

module.exports = { checkNodeVersion, resolvePlatformBinary, selectPlatform };
