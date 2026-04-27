const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");

const { HOMEBREW_FORMULA_PATH } = require("./render-homebrew-formula");

const workflowPath = path.join(
  __dirname,
  "..",
  ".github",
  "workflows",
  "devcontainer-release.yml",
);

const workflow = fs.readFileSync(workflowPath, "utf8");

function extractJob(name) {
  const startMatch = workflow.match(new RegExp(`^  ${name}:\\n`, "m"));
  assert.ok(startMatch, `expected ${name} job in devcontainer-release workflow`);

  const start = startMatch.index;
  const rest = workflow.slice(start + startMatch[0].length);
  const nextJob = rest.search(/^  [A-Za-z0-9_-]+:\n/m);
  return startMatch[0] + (nextJob === -1 ? rest : rest.slice(0, nextJob));
}

const homebrewJob = extractJob("homebrew");

assert.match(
  homebrewJob,
  /\s+needs:\s+\[prepare, build, release\]/,
  "Homebrew publish should wait until GitHub Release assets are live",
);
assert.match(
  homebrewJob,
  /\bHOMEBREW_TAP_TOKEN\b/,
  "Homebrew publish should use a dedicated tap token secret",
);
assert.match(
  homebrewJob,
  /repository:\s+jooh\/homebrew-devcontainer-rs-tap\b/,
  "Homebrew publish should check out Homebrew's default repository for the jooh/devcontainer-rs-tap shorthand",
);
assert.match(
  homebrewJob,
  /path:\s+tap\b/,
  "Homebrew publish should check out the tap repository under tap/",
);
assert.match(
  homebrewJob,
  /node build\/render-homebrew-formula\.js\b/,
  "Homebrew publish should use the repo-owned formula renderer",
);
assert.match(
  homebrewJob,
  new RegExp(`--output\\s+tap/${HOMEBREW_FORMULA_PATH.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}`),
  "Homebrew publish should write Formula/devcontainer-rs.rb in the tap checkout",
);
assert.match(
  homebrewJob,
  /git diff --cached --quiet/,
  "Homebrew publish should be idempotent when the formula is already current",
);
assert.match(
  homebrewJob,
  /git push origin HEAD:main/,
  "Homebrew publish should push the tap update to main",
);

console.log("[homebrew-publish-workflow] Homebrew tap publish workflow is wired.");
