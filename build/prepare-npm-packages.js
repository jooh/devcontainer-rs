const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { execFileSync } = require("node:child_process");

const runtimeConfig = require("../npm/runtime-config");

const ROOT = path.resolve(__dirname, "..");
const NPM_SOURCE_DIR = path.join(ROOT, "npm");
const REPOSITORY_URL = "https://github.com/jooh/devcontainer-rs.git";
const BUGS_URL = "https://github.com/jooh/devcontainer-rs/issues";

function parseArgs(argv) {
  const args = {};
  for (let index = 0; index < argv.length; index += 1) {
    const value = argv[index];
    if (!value.startsWith("--")) {
      continue;
    }
    const key = value.slice(2);
    args[key] = argv[index + 1];
    index += 1;
  }
  return args;
}

function mkTempDir(prefix) {
  return fs.mkdtempSync(path.join(os.tmpdir(), prefix));
}

function mkdirp(dirPath) {
  fs.mkdirSync(dirPath, { recursive: true });
}

function writeJson(filePath, value) {
  fs.writeFileSync(filePath, `${JSON.stringify(value, null, 2)}\n`, "utf8");
}

function copyFile(sourcePath, destinationPath, mode) {
  mkdirp(path.dirname(destinationPath));
  fs.copyFileSync(sourcePath, destinationPath);
  if (mode) {
    fs.chmodSync(destinationPath, mode);
  }
}

function recursiveFiles(rootDir) {
  const files = [];
  const stack = [rootDir];
  while (stack.length > 0) {
    const current = stack.pop();
    for (const entry of fs.readdirSync(current, { withFileTypes: true })) {
      const entryPath = path.join(current, entry.name);
      if (entry.isDirectory()) {
        stack.push(entryPath);
      } else if (entry.isFile()) {
        files.push(entryPath);
      }
    }
  }
  return files;
}

function archiveMatches(target, filePath, version) {
  const baseName = path.basename(filePath);
  const matchesTarget =
    baseName.includes(target.archiveSuffix) || baseName.includes(target.triple);
  if (!matchesTarget) {
    return false;
  }
  return baseName.includes(version) || baseName.startsWith("devcontainer-");
}

function findArchive(artifactsDir, target, version) {
  const candidates = recursiveFiles(artifactsDir).filter((filePath) =>
    archiveMatches(target, filePath, version),
  );
  const archive = candidates.find((filePath) =>
    /\.(tar\.gz|tar\.xz|tar\.zst|tar\.zstd)$/.test(filePath),
  );
  if (!archive) {
    throw new Error(
      `No archive found for ${target.target} in ${artifactsDir}. Expected a dist artifact containing ${target.archiveSuffix} or ${target.triple}.`,
    );
  }
  return archive;
}

function extractArchive(archivePath, tempDir) {
  if (archivePath.endsWith(".tar.gz")) {
    execFileSync("tar", ["-xzf", archivePath, "-C", tempDir]);
    return;
  }
  if (archivePath.endsWith(".tar.xz")) {
    execFileSync("tar", ["-xJf", archivePath, "-C", tempDir]);
    return;
  }
  if (archivePath.endsWith(".tar.zst") || archivePath.endsWith(".tar.zstd")) {
    execFileSync("tar", ["--zstd", "-xf", archivePath, "-C", tempDir]);
    return;
  }
  throw new Error(`Unsupported archive format for ${archivePath}`);
}

function findExtractedBinary(extractedDir) {
  const binary = recursiveFiles(extractedDir).find(
    (filePath) => path.basename(filePath) === "devcontainer",
  );
  if (!binary) {
    throw new Error(`No devcontainer binary found in ${extractedDir}`);
  }
  return binary;
}

function binaryOptionalDependencies(version) {
  return Object.fromEntries(
    Object.values(runtimeConfig.supportedTargets).map((target) => [
      target.packageName,
      version,
    ]),
  );
}

function createWrapperBin(filePath) {
  fs.writeFileSync(
    filePath,
    '#!/usr/bin/env node\nrequire("../launcher").run();\n',
    "utf8",
  );
  fs.chmodSync(filePath, 0o755);
}

function renderWrapperPackageJson(wrapper, version) {
  return {
    name: wrapper.packageName,
    version,
    description: wrapper.description,
    license: "MIT",
    repository: {
      type: "git",
      url: REPOSITORY_URL,
    },
    bugs: {
      url: BUGS_URL,
    },
    engines: {
      node: ">=20.0.0",
    },
    bin: wrapper.bin,
    files: ["bin", "launcher.js", "runtime-config.js"],
    optionalDependencies: binaryOptionalDependencies(version),
  };
}

function renderNativePackageJson(target, version) {
  const packageJson = {
    name: target.packageName,
    version,
    description: `Native ${target.target} build for devcontainer-rs`,
    license: "MIT",
    repository: {
      type: "git",
      url: REPOSITORY_URL,
    },
    bugs: {
      url: BUGS_URL,
    },
    os: [target.os],
    cpu: [target.cpu],
    files: ["bin/devcontainer"],
  };
  if (target.libc) {
    packageJson.libc = [target.libc];
  }
  return packageJson;
}

function stageWrapper(outputDir, wrapper, version) {
  const packageDir = path.join(outputDir, wrapper.packageSlug);
  mkdirp(path.join(packageDir, "bin"));
  writeJson(path.join(packageDir, "package.json"), renderWrapperPackageJson(wrapper, version));
  copyFile(path.join(NPM_SOURCE_DIR, "launcher.js"), path.join(packageDir, "launcher.js"));
  copyFile(
    path.join(NPM_SOURCE_DIR, "runtime-config.js"),
    path.join(packageDir, "runtime-config.js"),
  );
  for (const binPath of Object.values(wrapper.bin)) {
    createWrapperBin(path.join(packageDir, binPath));
  }
  return packageDir;
}

function stageNativePackage(outputDir, target, version, artifactsDir) {
  const packageDir = path.join(outputDir, target.packageSlug);
  const archivePath = findArchive(artifactsDir, target, version);
  const extractedDir = mkTempDir("devcontainer-rs-extracted-");
  extractArchive(archivePath, extractedDir);
  const binarySourcePath = findExtractedBinary(extractedDir);
  const binaryDestinationPath = path.join(packageDir, "bin", "devcontainer");

  mkdirp(path.dirname(binaryDestinationPath));
  copyFile(binarySourcePath, binaryDestinationPath, 0o755);
  writeJson(path.join(packageDir, "package.json"), renderNativePackageJson(target, version));
  return packageDir;
}

function readCargoVersion() {
  const cargoToml = fs.readFileSync(
    path.join(ROOT, "cmd", "devcontainer", "Cargo.toml"),
    "utf8",
  );
  const versionMatch = cargoToml.match(/^version = "([^"]+)"/m);
  if (!versionMatch) {
    throw new Error("Unable to determine crate version from cmd/devcontainer/Cargo.toml");
  }
  return versionMatch[1];
}

function prepareNpmPackages(options = {}) {
  const version = options.version || readCargoVersion();
  const artifactsDir = options.artifactsDir || path.join(ROOT, "target", "distrib");
  const outputDir = options.outputDir || path.join(ROOT, "dist", "npm");

  fs.rmSync(outputDir, { recursive: true, force: true });
  mkdirp(outputDir);

  const packageDirs = [];
  for (const wrapper of Object.values(runtimeConfig.wrapperPackages)) {
    packageDirs.push(stageWrapper(outputDir, wrapper, version));
  }
  for (const target of Object.values(runtimeConfig.supportedTargets)) {
    packageDirs.push(stageNativePackage(outputDir, target, version, artifactsDir));
  }

  return {
    version,
    outputDir,
    packageDirs,
  };
}

function main() {
  const args = parseArgs(process.argv.slice(2));
  const prepared = prepareNpmPackages({
    version: args.version,
    artifactsDir: args["artifacts-dir"],
    outputDir: args["output-dir"],
  });
  process.stdout.write(`${JSON.stringify(prepared, null, 2)}\n`);
}

module.exports = {
  prepareNpmPackages,
};

if (require.main === module) {
  main();
}
