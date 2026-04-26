const test = require("node:test");
const assert = require("node:assert/strict");

const { detectHostTarget } = require("./check-npm-packages");

test("npm package smoke selects the musl native package on linux x64 musl", () => {
  const target = detectHostTarget({
    platform: "linux",
    arch: "x64",
    libc: "musl",
  });

  assert.equal(target.target, "linux-x64-musl");
  assert.equal(
    target.packageName,
    "@devcontainer-rs/devcontainer-linux-x64-musl",
  );
});
