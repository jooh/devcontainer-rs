const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const test = require("node:test");

const {
  HOMEBREW_FORMULA_PATH,
  RELEASE_TARGETS,
  readSha256,
  renderHomebrewFormula,
} = require("./render-homebrew-formula");

test("renders the devcontainer-rs Homebrew formula", () => {
  const formula = renderHomebrewFormula({
    version: "1.2.3",
    shas: {
      "aarch64-apple-darwin": "a".repeat(64),
      "x86_64-apple-darwin": "b".repeat(64),
      "x86_64-unknown-linux-gnu": "c".repeat(64),
    },
  });

  assert.match(formula, /^class DevcontainerRs < Formula/m);
  assert.match(formula, /desc "Native Rust foundation for devcontainer CLI"/);
  assert.match(formula, /homepage "https:\/\/github.com\/jooh\/devcontainer-rs"/);
  assert.match(formula, /version "1\.2\.3"/);
  assert.match(formula, /license "MIT"/);
  assert.match(
    formula,
    /url "https:\/\/github.com\/jooh\/devcontainer-rs\/releases\/download\/devcontainer-v1\.2\.3\/devcontainer-aarch64-apple-darwin\.tar\.gz"/,
  );
  assert.match(
    formula,
    /url "https:\/\/github.com\/jooh\/devcontainer-rs\/releases\/download\/devcontainer-v1\.2\.3\/devcontainer-x86_64-apple-darwin\.tar\.gz"/,
  );
  assert.match(
    formula,
    /url "https:\/\/github.com\/jooh\/devcontainer-rs\/releases\/download\/devcontainer-v1\.2\.3\/devcontainer-x86_64-unknown-linux-gnu\.tar\.gz"/,
  );
  assert.match(formula, /depends_on arch: :x86_64/);
  assert.match(formula, /bin\.install "devcontainer"/);
  assert.match(
    formula,
    /assert_match version\.to_s, shell_output\("#\{bin\}\/devcontainer --version"\)/,
  );
  assert.equal(formula.endsWith("\n"), true);
});

test("reads cargo-dist sha256 files from the release artifact directory", () => {
  const tempDir = fs.mkdtempSync(path.join(os.tmpdir(), "devcontainer-rs-shas-"));
  const target = RELEASE_TARGETS.darwinArm64.triple;
  fs.writeFileSync(
    path.join(tempDir, `devcontainer-${target}.tar.gz.sha256`),
    `${"d".repeat(64)}  devcontainer-${target}.tar.gz\n`,
  );

  assert.equal(readSha256(tempDir, target), "d".repeat(64));
});

test("reads cargo-dist sha256 files from nested downloaded artifacts", () => {
  const tempDir = fs.mkdtempSync(path.join(os.tmpdir(), "devcontainer-rs-shas-"));
  const target = RELEASE_TARGETS.darwinArm64.triple;
  const checksumDir = path.join(tempDir, "target", "distrib");
  fs.mkdirSync(checksumDir, { recursive: true });
  fs.writeFileSync(
    path.join(checksumDir, `devcontainer-${target}.tar.gz.sha256`),
    `${"e".repeat(64)}  devcontainer-${target}.tar.gz\n`,
  );

  assert.equal(readSha256(tempDir, target), "e".repeat(64));
});

test("exports the tap formula path", () => {
  assert.equal(HOMEBREW_FORMULA_PATH, path.join("Formula", "devcontainer-rs.rb"));
});
