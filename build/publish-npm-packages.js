const fs = require("node:fs");
const path = require("node:path");
const { execFileSync } = require("node:child_process");

const CONFLICT_PATTERN =
  /You cannot publish over the previously published versions|cannot publish over existing version|EPUBLISHCONFLICT/;

function readPackageInfo(packageDir) {
  const manifestPath = path.join(packageDir, "package.json");
  const manifest = JSON.parse(fs.readFileSync(manifestPath, "utf8"));
  if (!manifest.name || !manifest.version) {
    throw new Error(`Expected name and version in ${manifestPath}`);
  }
  return {
    packageDir,
    manifestPath,
    name: manifest.name,
    version: manifest.version,
  };
}

function versionSpec(packageInfo) {
  return `${packageInfo.name}@${packageInfo.version}`;
}

function packageVersionExists(packageInfo, runNpm = defaultRunNpm) {
  try {
    runNpm(["view", versionSpec(packageInfo), "version"]);
    return true;
  } catch {
    return false;
  }
}

function defaultRunNpm(args) {
  return execFileSync("npm", args, {
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
  });
}

function publishPackage(packageDir, options = {}) {
  const runNpm = options.runNpm || defaultRunNpm;
  const log = options.log || (() => {});
  const access = options.access || "public";
  const packageInfo = readPackageInfo(packageDir);

  if (packageVersionExists(packageInfo, runNpm)) {
    log(`skipping ${versionSpec(packageInfo)} (already published)`);
    return { status: "skipped", packageInfo };
  }

  try {
    const output = runNpm(["publish", "--access", access, packageDir]);
    if (output) {
      log(output.trimEnd());
    }
    return { status: "published", packageInfo };
  } catch (error) {
    const combinedOutput = `${error.stdout || ""}${error.stderr || ""}`;
    if (CONFLICT_PATTERN.test(combinedOutput)) {
      log(`continuing after publish conflict: ${versionSpec(packageInfo)} is already on npm`);
      return { status: "skipped", packageInfo };
    }
    if (packageVersionExists(packageInfo, runNpm)) {
      log(`continuing after publish conflict: ${versionSpec(packageInfo)} is already on npm`);
      return { status: "skipped", packageInfo };
    }
    throw error;
  }
}

function publishPackageDirs(packageDirs, options = {}) {
  for (const packageDir of packageDirs) {
    publishPackage(packageDir, options);
  }
}

function main(argv = process.argv.slice(2)) {
  if (argv.length === 0) {
    throw new Error("Expected one or more package directories");
  }
  publishPackageDirs(argv, {
    log(message) {
      process.stdout.write(`${message}\n`);
    },
  });
}

module.exports = {
  publishPackage,
  publishPackageDirs,
  readPackageInfo,
  packageVersionExists,
};

if (require.main === module) {
  main();
}
