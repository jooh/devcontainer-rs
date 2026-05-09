const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");

const workflowPath = path.join(
  __dirname,
  "..",
  ".github",
  "workflows",
  "devcontainer-release.yml",
);

const workflow = fs.readFileSync(workflowPath, "utf8");
const buildJobMatch = workflow.match(/^  build:\n([\s\S]+?)^  release:/m);
const releaseJobMatch = workflow.match(/^  release:\n([\s\S]+?)^  pypi:/m);
const pypiJobMatch = workflow.match(/^  pypi:\n([\s\S]+?)^  npm:/m);
const npmJobMatch = workflow.match(/^  npm:\n([\s\S]+)$/m);

assert.ok(
  buildJobMatch,
  "expected build release job in devcontainer-release workflow",
);
assert.ok(
  releaseJobMatch,
  "expected GitHub release job in devcontainer-release workflow",
);
assert.ok(npmJobMatch, "expected npm release job in devcontainer-release workflow");
assert.ok(pypiJobMatch, "expected PyPI release job in devcontainer-release workflow");

const buildJob = buildJobMatch[0];
const releaseJob = releaseJobMatch[0];
const pypiJob = pypiJobMatch[0];
const npmJob = npmJobMatch[0];
const arm64MuslMatrixEntryMatch = buildJob.match(
  /- target:\s*linux-arm64-musl\n([\s\S]+?)(?=\n\s+- target:|\n\s+runs-on:)/,
);

assert.ok(
  arm64MuslMatrixEntryMatch,
  "release build matrix should include Linux arm64 musl artifacts",
);

const arm64MuslMatrixEntry = arm64MuslMatrixEntryMatch[0];

assert.match(
  buildJob,
  /tar -xzf "\$archive_path" -C "\$smoke_dir"\n\s+\.\/scripts\/standalone\/smoke\.sh "\$smoke_dir\/devcontainer-\$\{\{ matrix\.rust_target \}\}\/devcontainer"/,
  "cargo-dist smoke should run the binary inside the extracted top-level archive directory",
);
assert.doesNotMatch(
  buildJob,
  /\.\/scripts\/standalone\/smoke\.sh "\$smoke_dir\/devcontainer"/,
  "cargo-dist smoke should not assume the binary is extracted directly at the temp root",
);
assert.doesNotMatch(
  workflow,
  /\bNPM_TOKEN\b/,
  "npm trusted publishing should not depend on an npm token secret",
);
assert.doesNotMatch(
  workflow,
  /\bNODE_AUTH_TOKEN\b/,
  "npm trusted publishing should not wire NODE_AUTH_TOKEN into the publish job",
);
assert.match(
  npmJob,
  /\s+environment:\s*npm\b/,
  "npm publish job should use the npm GitHub environment",
);
assert.match(
  npmJob,
  /\s+permissions:\n(?:\s+.*\n)*?\s+id-token:\s+write\b/m,
  "npm publish job must request id-token: write for OIDC",
);
assert.match(
  npmJob,
  /\s+permissions:\n(?:\s+.*\n)*?\s+contents:\s+read\b/m,
  "npm publish job should only need read access to repository contents",
);
assert.match(
  npmJob,
  /uses:\s*actions\/setup-node@v6\b/,
  "npm publish job should use the current setup-node action",
);
assert.match(
  npmJob,
  /node-version:\s*'24'/,
  "npm publish job should use Node 24 so the bundled npm satisfies trusted publishing requirements",
);
assert.match(
  npmJob,
  /registry-url:\s*'https:\/\/registry\.npmjs\.org'/,
  "npm publish job should target the public npm registry",
);
assert.match(
  npmJob,
  /node build\/publish-npm-packages\.js --skip-unregistered /,
  "npm publish job should use the repo-owned idempotent npm publish helper",
);
assert.match(
  buildJob,
  /rust_target:\s*aarch64-unknown-linux-gnu\b/,
  "release build matrix should include Linux arm64 GNU artifacts",
);
assert.match(
  buildJob,
  /rust_target:\s*aarch64-unknown-linux-musl\b/,
  "release build matrix should include Linux arm64 musl artifacts",
);
assert.match(
  arm64MuslMatrixEntry,
  /pypi_wheel_builder:\s*none\b/,
  "Linux arm64 musl should not build a duplicate PyPI wheel basename",
);
assert.match(
  npmJob,
  /dist\/npm\/devcontainer-rs-devcontainer-linux-arm64-gnu\b/,
  "npm publish job should publish the Linux arm64 GNU native package",
);
assert.match(
  npmJob,
  /dist\/npm\/devcontainer-rs-devcontainer-linux-arm64-musl\b/,
  "npm publish job should publish the Linux arm64 musl native package",
);
assert.doesNotMatch(
  npmJob,
  /\bnpm publish --access public\b/,
  "npm publish job should not inline raw npm publish commands",
);
assert.match(
  pypiJob,
  /uses:\s*pypa\/gh-action-pypi-publish@release\/v1\b/,
  "PyPI publish job should use PyPA's trusted-publishing action",
);
assert.match(
  pypiJob,
  /find dist -type f -name '\*\.whl'/,
  "PyPI publish job should collect wheel files from the artifact download tree",
);
assert.doesNotMatch(
  releaseJob,
  /merge-multiple:\s*true\b/,
  "GitHub release job should preserve artifact directories to avoid duplicate path corruption",
);
assert.doesNotMatch(
  pypiJob,
  /merge-multiple:\s*true\b/,
  "PyPI publish job should preserve artifact directories to avoid duplicate path corruption",
);
assert.doesNotMatch(
  npmJob,
  /merge-multiple:\s*true\b/,
  "npm publish job should preserve artifact directories to avoid duplicate path corruption",
);
assert.match(
  releaseJob,
  /duplicate release artifact basename/,
  "GitHub release job should fail before uploading duplicate artifact basenames",
);
assert.match(
  releaseJob,
  /unzip -t "\$file"/,
  "GitHub release job should validate wheels before publishing release assets",
);
assert.match(
  pypiJob,
  /duplicate PyPI wheel basename/,
  "PyPI publish job should fail before copying duplicate wheel basenames",
);
assert.match(
  pypiJob,
  /unzip -t "\$wheel"/,
  "PyPI publish job should validate wheels before publishing to PyPI",
);
assert.match(
  pypiJob,
  /packages-dir:\s*pypi-dist\b/,
  "PyPI publish job should upload from a wheel-only directory",
);
assert.doesNotMatch(
  pypiJob,
  /packages-dir:\s*dist\b/,
  "PyPI publish job should not upload the mixed artifact download directory",
);
assert.match(
  pypiJob,
  /skip-existing:\s*true\b/,
  "PyPI publish job should skip already uploaded files after a partial release",
);
assert.doesNotMatch(
  pypiJob,
  /\buv publish\b/,
  "PyPI publish job should not use uv for the final upload path",
);
