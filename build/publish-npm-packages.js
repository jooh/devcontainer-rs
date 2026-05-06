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

function packageNameExists(packageInfo, runNpm = defaultRunNpm) {
  try {
    runNpm(["view", packageInfo.name, "name"]);
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

function pruneOptionalDependencies(packageInfo, availablePackageNames, log = () => {}) {
  const manifest = JSON.parse(fs.readFileSync(packageInfo.manifestPath, "utf8"));
  if (!manifest.optionalDependencies) {
    return;
  }

  const pruned = Object.fromEntries(
    Object.entries(manifest.optionalDependencies).filter(([name]) =>
      availablePackageNames.has(name),
    ),
  );
  const removed = Object.keys(manifest.optionalDependencies).filter(
    (name) => !availablePackageNames.has(name),
  );

  if (removed.length === 0) {
    return;
  }

  if (Object.keys(pruned).length === 0) {
    delete manifest.optionalDependencies;
  } else {
    manifest.optionalDependencies = pruned;
  }

  fs.writeFileSync(packageInfo.manifestPath, `${JSON.stringify(manifest, null, 2)}\n`, "utf8");
  log(
    `pruned unavailable optional dependencies from ${versionSpec(packageInfo)}: ${removed.join(", ")}`,
  );
}

function publishPackageDirs(packageDirs, options = {}) {
  const runNpm = options.runNpm || defaultRunNpm;
  const log = options.log || (() => {});
  const packageInfos = packageDirs.map(readPackageInfo);
  let publishablePackageNames = null;

  if (options.skipUnregistered) {
    publishablePackageNames = new Set();
    for (const packageInfo of packageInfos) {
      if (packageNameExists(packageInfo, runNpm)) {
        publishablePackageNames.add(packageInfo.name);
      } else {
        log(`skipping ${versionSpec(packageInfo)} (package is not registered on npm)`);
      }
    }

    for (const packageInfo of packageInfos) {
      if (publishablePackageNames.has(packageInfo.name)) {
        pruneOptionalDependencies(packageInfo, publishablePackageNames, log);
      }
    }
  }

  for (const packageInfo of packageInfos) {
    if (publishablePackageNames && !publishablePackageNames.has(packageInfo.name)) {
      continue;
    }
    publishPackage(packageInfo.packageDir, options);
  }
}

function parseArgs(argv) {
  const options = {
    packageDirs: [],
    skipUnregistered: false,
  };

  for (const arg of argv) {
    if (arg === "--skip-unregistered") {
      options.skipUnregistered = true;
    } else if (arg.startsWith("--")) {
      throw new Error(`Unknown option: ${arg}`);
    } else {
      options.packageDirs.push(arg);
    }
  }

  return options;
}

function main(argv = process.argv.slice(2)) {
  const options = parseArgs(argv);
  if (options.packageDirs.length === 0) {
    throw new Error("Expected one or more package directories");
  }
  publishPackageDirs(options.packageDirs, {
    skipUnregistered: options.skipUnregistered,
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
  packageNameExists,
};

if (require.main === module) {
  main();
}
