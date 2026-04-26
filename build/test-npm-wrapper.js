const test = require("node:test");
const assert = require("node:assert/strict");

const {
  detectLibc,
  resolveBinaryPackage,
  resolveInstalledBinary,
} = require("../npm/launcher");
const runtimeConfig = require("../npm/runtime-config");

test("maps darwin x64 to the darwin x64 native package", () => {
  const resolved = resolveBinaryPackage({
    platform: "darwin",
    arch: "x64",
  });
  assert.equal(resolved.target, "darwin-x64");
  assert.equal(
    resolved.packageName,
    "@devcontainer-rs/devcontainer-darwin-x64",
  );
});

test("maps darwin arm64 to the darwin arm64 native package", () => {
  const resolved = resolveBinaryPackage({
    platform: "darwin",
    arch: "arm64",
  });
  assert.equal(resolved.target, "darwin-arm64");
  assert.equal(
    resolved.packageName,
    "@devcontainer-rs/devcontainer-darwin-arm64",
  );
});

test("maps linux x64 glibc to the gnu native package", () => {
  const resolved = resolveBinaryPackage({
    platform: "linux",
    arch: "x64",
    libc: "gnu",
  });
  assert.equal(resolved.target, "linux-x64-gnu");
  assert.equal(
    resolved.packageName,
    "@devcontainer-rs/devcontainer-linux-x64-gnu",
  );
});

test("maps linux x64 musl to the musl native package", () => {
  const resolved = resolveBinaryPackage({
    platform: "linux",
    arch: "x64",
    libc: "musl",
  });
  assert.equal(resolved.target, "linux-x64-musl");
  assert.equal(
    resolved.packageName,
    "@devcontainer-rs/devcontainer-linux-x64-musl",
  );
});

test("detects musl when ldd reports on stderr and exits non-zero", () => {
  const detected = detectLibc({
    platform: "linux",
    env: {},
    report: {
      getReport() {
        return { header: {} };
      },
    },
    execFileSync(command, args) {
      if (command === "getconf" && args[0] === "GNU_LIBC_VERSION") {
        throw new Error("not glibc");
      }
      if (command === "ldd" && args[0] === "--version") {
        const error = new Error("musl ldd writes version to stderr");
        error.stderr = "musl libc (x86_64)\nVersion 1.2.5\n";
        throw error;
      }
      throw new Error(`unexpected command: ${command}`);
    },
  });

  assert.equal(detected, "musl");
});

test("rejects unsupported platforms with a helpful error", () => {
  assert.throws(
    () =>
      resolveBinaryPackage({
        platform: "win32",
        arch: "x64",
      }),
    /Unsupported platform/,
  );
});

test("fails cleanly when the expected native dependency is not installed", () => {
  assert.throws(
    () =>
      resolveInstalledBinary({
        packageRoot: process.cwd(),
        system: {
          platform: "darwin",
          arch: "arm64",
        },
        resolvePackageJson() {
          throw new Error("missing package");
        },
      }),
    /not installed/,
  );
});

test("runtime config optional dependency set matches supported targets", () => {
  const packageNames = Object.values(runtimeConfig.supportedTargets).map(
    (target) => target.packageName,
  );
  assert.deepEqual(packageNames.sort(), [
    "@devcontainer-rs/devcontainer-darwin-arm64",
    "@devcontainer-rs/devcontainer-darwin-x64",
    "@devcontainer-rs/devcontainer-linux-x64-gnu",
    "@devcontainer-rs/devcontainer-linux-x64-musl",
  ]);
});
