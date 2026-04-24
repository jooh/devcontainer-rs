const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { execFileSync } = require("node:child_process");

const { prepareNpmPackages } = require("./prepare-npm-packages");
const runtimeConfig = require("../npm/runtime-config");

const VERSION = "1.2.3";

function mkTempDir(prefix) {
  return fs.mkdtempSync(path.join(os.tmpdir(), prefix));
}

function writeExecutable(filePath, contents) {
  fs.mkdirSync(path.dirname(filePath), { recursive: true });
  fs.writeFileSync(filePath, contents, "utf8");
  fs.chmodSync(filePath, 0o755);
}

function createArchive(archivePath, version, target) {
  const rootDir = mkTempDir("devcontainer-rs-archive-");
  const archiveRoot = path.join(rootDir, `devcontainer-${version}-${target}`);
  const binaryPath = path.join(archiveRoot, "devcontainer");
  writeExecutable(
    binaryPath,
    `#!/bin/sh\nprintf '%s\\n' 'devcontainer ${version} ${target}'\n`,
  );
  fs.mkdirSync(path.dirname(archivePath), { recursive: true });
  execFileSync("tar", ["-czf", archivePath, "-C", rootDir, path.basename(archiveRoot)]);
}

function npmPack(packageDir, outputDir) {
  const npmHome = mkTempDir("devcontainer-rs-npm-home-");
  execFileSync("npm", ["pack", packageDir, "--pack-destination", outputDir], {
    stdio: "inherit",
    env: {
      ...process.env,
      HOME: npmHome,
      npm_config_cache: path.join(npmHome, ".npm"),
    },
  });
}

function installAndRun(tempDir, dependencies, command, expectedOutput) {
  const npmHome = mkTempDir("devcontainer-rs-npm-home-");
  const packageJsonPath = path.join(tempDir, "package.json");
  fs.writeFileSync(
    packageJsonPath,
    JSON.stringify(
      {
        name: "devcontainer-rs-test-app",
        version: "0.0.0",
        private: true,
        dependencies,
      },
      null,
      2,
    ),
  );
  console.error(`Installing smoke-test dependencies in ${tempDir}`);
  execFileSync(
    "npm",
    [
      "install",
      "--ignore-scripts",
      "--omit=optional",
      "--no-audit",
      "--fund=false",
      "--offline",
    ],
    {
      cwd: tempDir,
      stdio: "inherit",
      env: {
        ...process.env,
        HOME: npmHome,
        npm_config_cache: path.join(npmHome, ".npm"),
      },
    },
  );
  console.error(`Running smoke-test command: ${command.join(" ")}`);
  const output = execFileSync(command[0], command.slice(1), {
    cwd: tempDir,
    encoding: "utf8",
    env: {
      ...process.env,
      HOME: npmHome,
      npm_config_cache: path.join(npmHome, ".npm"),
    },
  }).trim();
  assert.equal(output, expectedOutput);
}

function detectHostTarget() {
  if (process.platform === "darwin" && process.arch === "arm64") {
    return runtimeConfig.supportedTargets["darwin-arm64"];
  }
  if (process.platform === "darwin" && process.arch === "x64") {
    return runtimeConfig.supportedTargets["darwin-x64"];
  }
  if (process.platform === "linux" && process.arch === "x64") {
    return runtimeConfig.supportedTargets["linux-x64-gnu"];
  }
  throw new Error(`unsupported test host: ${process.platform}/${process.arch}`);
}

function main() {
  const artifactsDir = mkTempDir("devcontainer-rs-artifacts-");
  const outputDir = mkTempDir("devcontainer-rs-packages-");
  const packedDir = mkTempDir("devcontainer-rs-packed-");

  for (const target of Object.values(runtimeConfig.supportedTargets)) {
    createArchive(
      path.join(artifactsDir, `devcontainer-${target.triple}.tar.gz`),
      VERSION,
      target.target,
    );
  }

  const prepared = prepareNpmPackages({
    version: VERSION,
    artifactsDir,
    outputDir,
  });

  for (const packageDir of prepared.packageDirs) {
    npmPack(packageDir, packedDir);
  }

  const hostTarget = detectHostTarget();
  const nativeTarball = path.join(
    packedDir,
    `${hostTarget.packageSlug}-${VERSION}.tgz`,
  );
  const wrapperTarball = path.join(packedDir, `devcontainer-rs-${VERSION}.tgz`);
  const scopedWrapperTarball = path.join(
    packedDir,
    `devcontainer-rs-cli-${VERSION}.tgz`,
  );

  installAndRun(
    mkTempDir("devcontainer-rs-install-"),
    {
      "devcontainer-rs": `file:${wrapperTarball}`,
      [hostTarget.packageName]: `file:${nativeTarball}`,
    },
    ["npm", "exec", "--no", "--", "devcontainer-rs", "--version"],
    `devcontainer ${VERSION} ${hostTarget.target}`,
  );

  const scopedInstallDir = mkTempDir("devcontainer-rs-install-scoped-");
  installAndRun(
    scopedInstallDir,
    {
      "@devcontainer-rs/cli": `file:${scopedWrapperTarball}`,
      [hostTarget.packageName]: `file:${nativeTarball}`,
    },
    [
      path.join(scopedInstallDir, "node_modules", ".bin", "devcontainer"),
      "--version",
    ],
    `devcontainer ${VERSION} ${hostTarget.target}`,
  );

  const directBinaryOutput = execFileSync(
    path.join(scopedInstallDir, "node_modules", ".bin", "devcontainer"),
    ["--version"],
    {
      encoding: "utf8",
    },
  ).trim();
  assert.equal(directBinaryOutput, `devcontainer ${VERSION} ${hostTarget.target}`);
}

main();
