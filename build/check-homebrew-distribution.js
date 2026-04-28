const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");

function readRepoFile(...segments) {
  return fs.readFileSync(path.join(__dirname, "..", ...segments), "utf8");
}

const workflow = readRepoFile(".github", "workflows", "devcontainer-release.yml");
const distributionDocs = readRepoFile("docs", "standalone", "distribution.md");
const readme = readRepoFile("README.md");
const gitmodules = readRepoFile(".gitmodules");

assert.match(workflow, /^  release:\n/m, "release workflow should still publish GitHub Release assets");
assert.doesNotMatch(
  workflow,
  /^  homebrew:\n/m,
  "source release workflow should not own Homebrew tap publication",
);
assert.doesNotMatch(
  workflow,
  /\bHOMEBREW_TAP_TOKEN\b/,
  "source release workflow should not require a cross-repo Homebrew token",
);
assert.doesNotMatch(
  workflow,
  /repository:\s+jooh\/homebrew-devcontainer-rs\b/,
  "source release workflow should not check out the tap repository",
);
assert.doesNotMatch(
  workflow,
  /build\/render-homebrew-formula\.js\b/,
  "source release workflow should not render the tap-owned formula",
);

assert.match(
  distributionDocs,
  /tap repository owns formula publishing/,
  "distribution docs should describe tap-owned formula publishing",
);
assert.match(
  distributionDocs,
  /jooh\/homebrew-devcontainer-rs/,
  "distribution docs should name the backing tap repository",
);
assert.doesNotMatch(
  distributionDocs,
  /\bHOMEBREW_TAP_TOKEN\b/,
  "distribution docs should not require a Homebrew tap token",
);
assert.match(
  readme,
  /brew install jooh\/devcontainer-rs\/devcontainer-rs/,
  "README should document the correct Homebrew install command",
);
assert.match(
  gitmodules,
  /url = https:\/\/github\.com\/jooh\/homebrew-devcontainer-rs\b/,
  "tap submodule should point at Homebrew's default repository for jooh/devcontainer-rs",
);

console.log("[homebrew-distribution] Source repo delegates Homebrew publication to the tap.");
