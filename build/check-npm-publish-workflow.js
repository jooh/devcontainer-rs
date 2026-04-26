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
const npmJobMatch = workflow.match(/^  npm:\n([\s\S]+)$/m);

assert.ok(npmJobMatch, "expected npm release job in devcontainer-release workflow");

const npmJob = npmJobMatch[0];

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
  /node build\/publish-npm-packages\.js /,
  "npm publish job should use the repo-owned idempotent npm publish helper",
);
assert.doesNotMatch(
  npmJob,
  /\bnpm publish --access public\b/,
  "npm publish job should not inline raw npm publish commands",
);
