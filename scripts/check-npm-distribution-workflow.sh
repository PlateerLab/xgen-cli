#!/bin/sh

set -eu

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd -P)
repo_root=$(CDPATH= cd -- "$script_dir/.." && pwd -P)
if [ "$#" -gt 1 ]; then
    echo "usage: $0 [release-workflow]" >&2
    exit 2
fi
workflow=${1:-"$repo_root/.github/workflows/release.yml"}

python3 - \
    "$workflow" \
    "$repo_root/npm/scripts/publish.mjs" \
    "$repo_root/npm/scripts/smoke.mjs" \
    "$repo_root/.github/workflows/ci.yml" <<'PY'
from pathlib import Path
import re
import sys

workflow = Path(sys.argv[1])
publisher = Path(sys.argv[2])
smoke = Path(sys.argv[3])
lines = workflow.read_text(encoding="utf-8").splitlines()
text = "\n".join(lines)
publisher_text = publisher.read_text(encoding="utf-8")
smoke_text = smoke.read_text(encoding="utf-8")
ci_text = Path(sys.argv[4]).read_text(encoding="utf-8")


def fail(message: str) -> None:
    raise SystemExit(f"{workflow}: {message}")


for secret_marker in ("_authToken", "npm_password"):
    if secret_marker.lower() in text.lower():
        fail(f"literal npm credential configuration is forbidden: {secret_marker}")

if re.search(r"npm_[A-Za-z0-9]{20,}", text):
    fail("literal npm token value is forbidden")

preflight = "node npm/scripts/preflight-release.mjs"
fast = ci_text.split("\n  quality:", 1)[0]
quality = text.split("\n  build:", 1)[0]
for body, later, label in (
    (fast, "rustup toolchain install", "PR Rust toolchain setup"),
    (fast, "npm ci --ignore-scripts --prefix npm", "PR npm dependencies"),
    (quality, "npm ci --ignore-scripts --prefix npm", "release npm dependencies"),
    (quality, "cargo clippy", "release compilation"),
):
    if body.count(preflight) != 1 or later not in body or body.index(preflight) > body.index(later):
        fail(f"assembly preflight must run before {label}")
assembly = text.split("\n  assemble:", 1)[1].split("\n  publish:", 1)[0]
if assembly.count('node npm/scripts/assemble-release.mjs --directory release --tag "$RELEASE_TAG"') != 1:
    fail("release must use the shared assembly entrypoint")
if "node npm/scripts/pack.mjs" in assembly or "node npm/scripts/verify-release.mjs" in assembly:
    fail("release assembly must not duplicate shared packaging commands")

job_starts = [index for index, line in enumerate(lines) if line == "  publish-npm:"]
if len(job_starts) != 1:
    fail(f"expected one publish-npm job, found {len(job_starts)}")
job_start = job_starts[0]
job_end = next(
    (
        index
        for index in range(job_start + 1, len(lines))
        if re.fullmatch(r"  [A-Za-z0-9_-]+:", lines[index])
    ),
    len(lines),
)
job = "\n".join(lines[job_start:job_end])

required_job_fragments = (
    "    needs: publish",
    "      contents: read",
    "      id-token: write",
    "XGEN_NPM_PUBLISH_ENABLED",
    "test \"$NPM_PUBLISH_ENABLED\" = \"true\"",
    "node-version: 24.20.0",
    "actions/setup-node@249970729cb0ef3589644e2896645e5dc5ba9c38",
    "node npm/scripts/verify-release.mjs",
    "node npm/scripts/publish.mjs",
)
for fragment in required_job_fragments:
    if fragment not in job:
        fail(f"publish-npm job is missing required contract: {fragment}")

if "contents: write" in job:
    fail("publish-npm job must not have GitHub contents write permission")
if job.count("node npm/scripts/publish.mjs") != 1:
    fail("publish-npm job must invoke the npm publisher exactly once")
if job.index("node npm/scripts/verify-release.mjs") > job.index("node npm/scripts/publish.mjs"):
    fail("npm release bundle verification must happen before publication")

for forbidden in ("NPM_TOKEN", "NODE_AUTH_TOKEN", "_authToken", "registry-url:"):
    if forbidden in text:
        fail(f"npm publishing must use trusted publishing without token configuration: {forbidden}")
npm_step_marker = "- name: Install pinned npm for OIDC publish and dist-tag"
publish_step_marker = "- name: Publish platform packages then launcher"
for marker in (npm_step_marker, publish_step_marker):
    if marker not in job:
        fail(f"publish-npm job is missing step: {marker}")
npm_step = job[job.index(npm_step_marker) : job.index(publish_step_marker)]
for fragment in (
    "NPM_VERSION: 11.21.3",
    "NPM_INTEGRITY: sha512-",
    'test "$actual" = "$NPM_INTEGRITY"',
    'test "$(npm --version)" = "$NPM_VERSION"',
):
    if fragment not in npm_step:
        fail(f"pinned npm step is missing integrity contract: {fragment}")
if 'test "$(npm --version)" = 11.21.3' not in job[job.index(publish_step_marker) :]:
    fail("npm publish step must confirm the pinned npm version")
if job.index("node npm/scripts/verify-release.mjs") > job.index(publish_step_marker):
    fail("npm release bundle verification must happen before publication")

gate_step = text.find("- name: Require npm trusted publishing readiness")
build_job = text.find("\n  build:")
if gate_step < 0 or build_job < 0 or gate_step > build_job:
    fail("npm readiness must fail closed in the release quality gate before native builds")

verify_starts = [index for index, line in enumerate(lines) if line == "  verify-published-npm:"]
if len(verify_starts) != 1:
    fail(f"expected one verify-published-npm job, found {len(verify_starts)}")
verify_start = verify_starts[0]
verify_job = "\n".join(lines[verify_start:])
for fragment in (
    "    needs: publish-npm",
    "x86_64-unknown-linux-musl",
    "aarch64-unknown-linux-musl",
    "x86_64-apple-darwin",
    "aarch64-apple-darwin",
    "x86_64-pc-windows-msvc",
    "--include=optional --ignore-scripts",
    '"@xgen-ai/cli@$version"',
    '"@xgen-ai/cli@$Version"',
    "printf '/status\\n/exit\\n'",
    "status: idle",
    'npm uninstall --global --prefix "$test_root/install"',
    "& npm.cmd uninstall --global --prefix $InstallRoot",
    "npm uninstall left package artifacts",
):
    if fragment not in verify_job:
        fail(f"published npm verification is missing: {fragment}")

for fragment in (
    "const { launcher, reports, version } = await verifyRelease(directory, tag);",
    "for (const report of publicationOrder(reports, launcher.name))",
    "...reports.filter(({ name }) => name !== launcherName)",
    "launcherReports[0]",
    "'--provenance'",
    "shell: false",
    "dist.integrity",
    "dist.attestations",
):
    if fragment not in publisher_text:
        fail(f"npm publisher is missing required safety contract: {fragment}")

for fragment in (
    "const uninstall = npmInvocation([",
    "await assert.rejects(lstat(installedLauncher)",
    "await assert.rejects(lstat(installedPlatform)",
    "npm install/reinstall/remove smoke: PASS",
):
    if fragment not in smoke_text:
        fail(f"npm package smoke is missing lifecycle verification: {fragment}")

install_invocation = "await run(install.command, install.args, { env: npmEnvironment });"
if smoke_text.count(install_invocation) != 1:
    fail("npm package smoke must install the exact package")
if smoke_text.count("await run(defaultInstall.command, defaultInstall.args, { env: npmEnvironment });") != 1:
    fail("npm package smoke must also install the default package")

print("npm release workflow contract: PASS")
PY
