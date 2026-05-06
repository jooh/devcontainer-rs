const fs = require("node:fs");
const path = require("node:path");
const { execFileSync, spawn } = require("node:child_process");

const runtimeConfig = require("./runtime-config");

function outputText(value) {
  if (!value) {
    return "";
  }
  if (Buffer.isBuffer(value)) {
    return value.toString("utf8");
  }
  return String(value);
}

function parseLibcOutput(output) {
  if (/musl/i.test(output)) {
    return "musl";
  }
  if (/glibc|gnu libc|gnu c library/i.test(output)) {
    return "gnu";
  }
  return null;
}

function detectLibc(system = {}) {
  if ((system.platform || process.platform) !== "linux") {
    return null;
  }

  if (system.libc) {
    return system.libc;
  }

  const env = system.env || process.env;
  if (env.DEVCONTAINER_RS_LIBC) {
    return env.DEVCONTAINER_RS_LIBC;
  }

  const report = system.report || process.report;
  if (report && typeof report.getReport === "function") {
    const glibcVersion = report.getReport()?.header?.glibcVersionRuntime;
    if (glibcVersion) {
      return "gnu";
    }
  }

  const runCommand = system.execFileSync || execFileSync;
  try {
    const output = runCommand("getconf", ["GNU_LIBC_VERSION"], {
      encoding: "utf8",
      stdio: ["ignore", "pipe", "ignore"],
    });
    const libc = parseLibcOutput(output);
    if (libc) {
      return libc;
    }
  } catch (_error) {
    // fall through to ldd parsing
  }

  try {
    const output = runCommand("ldd", ["--version"], {
      encoding: "utf8",
      stdio: ["ignore", "pipe", "pipe"],
    });
    const libc = parseLibcOutput(output);
    if (libc) {
      return libc;
    }
  } catch (error) {
    const libc = parseLibcOutput(
      `${outputText(error.stdout)}\n${outputText(error.stderr)}`,
    );
    if (libc) {
      return libc;
    }
  }

  return "gnu";
}

function supportedPlatformList() {
  return Object.keys(runtimeConfig.supportedTargets).join(", ");
}

function resolveBinaryPackage(system = {}) {
  const platform = system.platform || process.platform;
  const arch = system.arch || process.arch;

  if (platform === "darwin" && arch === "x64") {
    return runtimeConfig.supportedTargets["darwin-x64"];
  }
  if (platform === "darwin" && arch === "arm64") {
    return runtimeConfig.supportedTargets["darwin-arm64"];
  }
  if (platform === "linux" && (arch === "x64" || arch === "arm64")) {
    const targetArch = arch === "x64" ? "x64" : "arm64";
    const libc = detectLibc(system);
    if (libc === "musl") {
      return runtimeConfig.supportedTargets[`linux-${targetArch}-musl`];
    }
    return runtimeConfig.supportedTargets[`linux-${targetArch}-gnu`];
  }

  throw new Error(
    `Unsupported platform ${platform}/${arch}. Supported platforms: ${supportedPlatformList()}`,
  );
}

function resolveInstalledBinary(options = {}) {
  const packageRoot = options.packageRoot || __dirname;
  const system = options.system || {};
  const target = resolveBinaryPackage(system);
  const resolvePackageJson =
    options.resolvePackageJson ||
    ((packageName) =>
      require.resolve(`${packageName}/package.json`, {
        paths: [packageRoot],
      }));

  let packageJsonPath;
  try {
    packageJsonPath = resolvePackageJson(target.packageName);
  } catch (error) {
    throw new Error(
      `The native package ${target.packageName} for ${target.target} is not installed.`,
      { cause: error },
    );
  }

  const binaryPath = path.join(
    path.dirname(packageJsonPath),
    runtimeConfig.binaryRelativePath,
  );
  try {
    fs.accessSync(binaryPath, fs.constants.X_OK);
  } catch (error) {
    throw new Error(
      `The native binary ${binaryPath} is missing or not executable.`,
      { cause: error },
    );
  }

  return {
    ...target,
    binaryPath,
  };
}

function run(argv = process.argv.slice(2)) {
  let resolved;
  try {
    resolved = resolveInstalledBinary();
  } catch (error) {
    console.error(error.message);
    process.exit(1);
  }

  const child = spawn(resolved.binaryPath, argv, {
    stdio: "inherit",
  });

  child.on("error", (error) => {
    console.error(error.message);
    process.exit(1);
  });

  child.on("exit", (code, signal) => {
    if (signal) {
      process.kill(process.pid, signal);
      return;
    }
    process.exit(code ?? 1);
  });
}

module.exports = {
  detectLibc,
  resolveBinaryPackage,
  resolveInstalledBinary,
  run,
};

if (require.main === module) {
  run();
}
