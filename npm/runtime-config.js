const supportedTargets = {
  "darwin-x64": {
    target: "darwin-x64",
    triple: "x86_64-apple-darwin",
    archiveSuffix: "darwin-x64",
    packageName: "@devcontainer-rs/devcontainer-darwin-x64",
    packageSlug: "devcontainer-rs-devcontainer-darwin-x64",
    os: "darwin",
    cpu: "x64",
  },
  "darwin-arm64": {
    target: "darwin-arm64",
    triple: "aarch64-apple-darwin",
    archiveSuffix: "darwin-arm64",
    packageName: "@devcontainer-rs/devcontainer-darwin-arm64",
    packageSlug: "devcontainer-rs-devcontainer-darwin-arm64",
    os: "darwin",
    cpu: "arm64",
  },
  "linux-x64-gnu": {
    target: "linux-x64-gnu",
    triple: "x86_64-unknown-linux-gnu",
    archiveSuffix: "linux-x64-gnu",
    packageName: "@devcontainer-rs/devcontainer-linux-x64-gnu",
    packageSlug: "devcontainer-rs-devcontainer-linux-x64-gnu",
    os: "linux",
    cpu: "x64",
    libc: "glibc",
  },
  "linux-x64-musl": {
    target: "linux-x64-musl",
    triple: "x86_64-unknown-linux-musl",
    archiveSuffix: "linux-x64-musl",
    packageName: "@devcontainer-rs/devcontainer-linux-x64-musl",
    packageSlug: "devcontainer-rs-devcontainer-linux-x64-musl",
    os: "linux",
    cpu: "x64",
    libc: "musl",
  },
};

const wrapperPackages = {
  "devcontainer-rs": {
    packageName: "devcontainer-rs",
    packageSlug: "devcontainer-rs",
    description: "Rust-native devcontainer CLI wrapper package",
    bin: {
      "devcontainer-rs": "bin/devcontainer-rs.js",
      devcontainer: "bin/devcontainer-rs.js",
    },
  },
  cli: {
    packageName: "@devcontainer-rs/cli",
    packageSlug: "devcontainer-rs-cli",
    description: "Rust-native devcontainer CLI wrapper package for the upstream @devcontainers/cli shape",
    bin: {
      devcontainer: "bin/devcontainer.js",
    },
  },
};

module.exports = {
  binaryRelativePath: "bin/devcontainer",
  supportedTargets,
  wrapperPackages,
};
