#!/usr/bin/env node

const fs = require("node:fs");
const path = require("node:path");

const HOMEBREW_FORMULA_PATH = path.join("Formula", "devcontainer-rs.rb");
const RELEASE_REPOSITORY = "jooh/devcontainer-rs";
const RELEASE_TARGETS = Object.freeze({
  darwinArm64: Object.freeze({
    triple: "aarch64-apple-darwin",
  }),
  darwinX64: Object.freeze({
    triple: "x86_64-apple-darwin",
  }),
  linuxX64Gnu: Object.freeze({
    triple: "x86_64-unknown-linux-gnu",
  }),
});

function archiveName(triple) {
  return `devcontainer-${triple}.tar.gz`;
}

function releaseUrl(repository, tag, triple) {
  return `https://github.com/${repository}/releases/download/${tag}/${archiveName(triple)}`;
}

function parseArgs(argv) {
  const args = {};

  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg === "--help" || arg === "-h") {
      args.help = true;
      continue;
    }

    if (!arg.startsWith("--")) {
      throw new Error(`unexpected argument: ${arg}`);
    }

    const key = arg.slice(2);
    const value = argv[index + 1];
    if (!value || value.startsWith("--")) {
      throw new Error(`missing value for ${arg}`);
    }
    args[key] = value;
    index += 1;
  }

  return args;
}

function usage() {
  return [
    "Usage: node build/render-homebrew-formula.js --version <version> --artifacts-dir <dir> --output <path>",
    "",
    "Options:",
    "  --version        Release version without the tag prefix, for example 1.2.3",
    "  --tag            Release tag; defaults to devcontainer-v<version>",
    "  --artifacts-dir  Directory containing cargo-dist .tar.gz.sha256 files, searched recursively",
    "  --output         Formula path to write, usually tap/Formula/devcontainer-rs.rb",
  ].join("\n");
}

function findChecksumFile(artifactsDir, filename) {
  const directPath = path.join(artifactsDir, filename);
  if (fs.existsSync(directPath)) {
    return directPath;
  }

  const pending = [artifactsDir];
  while (pending.length > 0) {
    const directory = pending.pop();
    const entries = fs.readdirSync(directory, { withFileTypes: true }).sort((left, right) =>
      left.name.localeCompare(right.name),
    );

    for (const entry of entries) {
      const entryPath = path.join(directory, entry.name);
      if (entry.isDirectory()) {
        pending.push(entryPath);
      } else if (entry.isFile() && entry.name === filename) {
        return entryPath;
      }
    }
  }

  throw new Error(`missing checksum file ${filename} under ${artifactsDir}`);
}

function readSha256(artifactsDir, triple) {
  const shaPath = findChecksumFile(artifactsDir, `${archiveName(triple)}.sha256`);
  const content = fs.readFileSync(shaPath, "utf8").trim();
  const match = content.match(/^([a-f0-9]{64})\b/i);
  if (!match) {
    throw new Error(`could not parse SHA-256 from ${shaPath}`);
  }
  return match[1].toLowerCase();
}

function shasFromArtifacts(artifactsDir) {
  return Object.fromEntries(
    Object.values(RELEASE_TARGETS).map(({ triple }) => [
      triple,
      readSha256(artifactsDir, triple),
    ]),
  );
}

function requireSha(shas, triple) {
  const sha = shas[triple];
  if (!sha) {
    throw new Error(`missing SHA-256 for ${triple}`);
  }
  if (!/^[a-f0-9]{64}$/.test(sha)) {
    throw new Error(`invalid SHA-256 for ${triple}: ${sha}`);
  }
  return sha;
}

function renderHomebrewFormula({
  version,
  tag = `devcontainer-v${version}`,
  repository = RELEASE_REPOSITORY,
  shas,
}) {
  if (!version) {
    throw new Error("version is required");
  }
  if (!shas) {
    throw new Error("shas are required");
  }

  const darwinArm64 = RELEASE_TARGETS.darwinArm64.triple;
  const darwinX64 = RELEASE_TARGETS.darwinX64.triple;
  const linuxX64Gnu = RELEASE_TARGETS.linuxX64Gnu.triple;

  return `class DevcontainerRs < Formula
  desc "Native Rust foundation for devcontainer CLI"
  homepage "https://github.com/${repository}"
  version "${version}"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "${releaseUrl(repository, tag, darwinArm64)}"
      sha256 "${requireSha(shas, darwinArm64)}"
    else
      url "${releaseUrl(repository, tag, darwinX64)}"
      sha256 "${requireSha(shas, darwinX64)}"
    end
  end

  on_linux do
    depends_on arch: :x86_64

    url "${releaseUrl(repository, tag, linuxX64Gnu)}"
    sha256 "${requireSha(shas, linuxX64Gnu)}"
  end

  def install
    bin.install "devcontainer"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/devcontainer --version")
  end
end
`;
}

function writeHomebrewFormula({ version, tag, artifactsDir, output }) {
  const formula = renderHomebrewFormula({
    version,
    tag,
    shas: shasFromArtifacts(artifactsDir),
  });

  fs.mkdirSync(path.dirname(output), { recursive: true });
  fs.writeFileSync(output, formula, "utf8");
  return formula;
}

function main() {
  const args = parseArgs(process.argv.slice(2));
  if (args.help) {
    console.log(usage());
    return;
  }

  const version = args.version;
  const artifactsDir = args["artifacts-dir"];
  const output = args.output;
  if (!version || !artifactsDir || !output) {
    throw new Error(`missing required arguments\n\n${usage()}`);
  }

  writeHomebrewFormula({
    version,
    tag: args.tag,
    artifactsDir,
    output,
  });
}

if (require.main === module) {
  main();
}

module.exports = {
  HOMEBREW_FORMULA_PATH,
  RELEASE_TARGETS,
  archiveName,
  readSha256,
  renderHomebrewFormula,
  shasFromArtifacts,
  writeHomebrewFormula,
};
