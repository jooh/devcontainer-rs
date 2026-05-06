const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const test = require("node:test");

const {
  publishPackageDirs,
} = require("./publish-npm-packages");

function mkTempDir(prefix) {
  return fs.mkdtempSync(path.join(os.tmpdir(), prefix));
}

function writePackage(dir, name, version) {
  fs.mkdirSync(dir, { recursive: true });
  fs.writeFileSync(
    path.join(dir, "package.json"),
    `${JSON.stringify({ name, version }, null, 2)}\n`,
    "utf8",
  );
}

function readPackage(dir) {
  return JSON.parse(fs.readFileSync(path.join(dir, "package.json"), "utf8"));
}

function makeConflictError(message) {
  const error = new Error(message);
  error.stdout = "";
  error.stderr = message;
  return error;
}

test("skips publish when the package version already exists", () => {
  const packageDir = mkTempDir("devcontainer-rs-publish-skip-");
  writePackage(packageDir, "@devcontainer-rs/example", "1.2.3");

  const calls = [];
  publishPackageDirs([packageDir], {
    runNpm(args) {
      calls.push(args);
      if (args[0] === "view") {
        return "1.2.3";
      }
      throw new Error("publish should not run for an already-published package");
    },
    log() {},
  });

  assert.deepEqual(calls, [["view", "@devcontainer-rs/example@1.2.3", "version"]]);
});

test("continues when npm publish reports a publish conflict", () => {
  const packageDir = mkTempDir("devcontainer-rs-publish-conflict-");
  writePackage(packageDir, "@devcontainer-rs/example", "1.2.3");

  const calls = [];
  publishPackageDirs([packageDir], {
    runNpm(args) {
      calls.push(args);
      if (args[0] === "view") {
        throw new Error("not found");
      }
      if (args[0] === "publish") {
        throw makeConflictError(
          "npm error You cannot publish over the previously published versions: 1.2.3.",
        );
      }
      throw new Error(`unexpected npm args: ${args.join(" ")}`);
    },
    log() {},
  });

  assert.deepEqual(calls, [
    ["view", "@devcontainer-rs/example@1.2.3", "version"],
    ["publish", "--access", "public", packageDir],
  ]);
});

test("skips unregistered packages and prunes unavailable optional dependencies", () => {
  const existingNativeDir = mkTempDir("devcontainer-rs-publish-existing-native-");
  const missingNativeDir = mkTempDir("devcontainer-rs-publish-missing-native-");
  const wrapperDir = mkTempDir("devcontainer-rs-publish-wrapper-");

  writePackage(existingNativeDir, "@devcontainer-rs/devcontainer-linux-x64-gnu", "1.2.3");
  writePackage(missingNativeDir, "@devcontainer-rs/devcontainer-linux-arm64-gnu", "1.2.3");
  fs.mkdirSync(wrapperDir, { recursive: true });
  fs.writeFileSync(
    path.join(wrapperDir, "package.json"),
    `${JSON.stringify(
      {
        name: "@devcontainer-rs/cli",
        version: "1.2.3",
        optionalDependencies: {
          "@devcontainer-rs/devcontainer-linux-x64-gnu": "1.2.3",
          "@devcontainer-rs/devcontainer-linux-arm64-gnu": "1.2.3",
        },
      },
      null,
      2,
    )}\n`,
    "utf8",
  );

  const calls = [];
  publishPackageDirs([existingNativeDir, missingNativeDir, wrapperDir], {
    skipUnregistered: true,
    runNpm(args) {
      calls.push(args);
      if (args[0] === "view" && args.length === 3 && args[2] === "name") {
        if (args[1] === "@devcontainer-rs/devcontainer-linux-arm64-gnu") {
          throw new Error("not found");
        }
        return args[1];
      }
      if (args[0] === "view" && args.length === 3 && args[2] === "version") {
        throw new Error("new version not published yet");
      }
      if (args[0] === "publish") {
        return "published";
      }
      throw new Error(`unexpected npm args: ${args.join(" ")}`);
    },
    log() {},
  });

  assert.deepEqual(readPackage(wrapperDir).optionalDependencies, {
    "@devcontainer-rs/devcontainer-linux-x64-gnu": "1.2.3",
  });
  assert.deepEqual(calls, [
    ["view", "@devcontainer-rs/devcontainer-linux-x64-gnu", "name"],
    ["view", "@devcontainer-rs/devcontainer-linux-arm64-gnu", "name"],
    ["view", "@devcontainer-rs/cli", "name"],
    ["view", "@devcontainer-rs/devcontainer-linux-x64-gnu@1.2.3", "version"],
    ["publish", "--access", "public", existingNativeDir],
    ["view", "@devcontainer-rs/cli@1.2.3", "version"],
    ["publish", "--access", "public", wrapperDir],
  ]);
});

test("publishes packages in the given order", () => {
  const firstDir = mkTempDir("devcontainer-rs-publish-order-");
  const secondDir = mkTempDir("devcontainer-rs-publish-order-");
  writePackage(firstDir, "@devcontainer-rs/first", "1.2.3");
  writePackage(secondDir, "@devcontainer-rs/second", "1.2.3");

  const calls = [];
  publishPackageDirs([firstDir, secondDir], {
    runNpm(args) {
      calls.push(args);
      if (args[0] === "view") {
        throw new Error("not found");
      }
      if (args[0] === "publish") {
        return "published";
      }
      throw new Error(`unexpected npm args: ${args.join(" ")}`);
    },
    log() {},
  });

  assert.deepEqual(calls, [
    ["view", "@devcontainer-rs/first@1.2.3", "version"],
    ["publish", "--access", "public", firstDir],
    ["view", "@devcontainer-rs/second@1.2.3", "version"],
    ["publish", "--access", "public", secondDir],
  ]);
});
