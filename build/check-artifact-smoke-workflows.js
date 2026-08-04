const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");

const root = path.join(__dirname, "..");

function readWorkflow(name) {
  return fs.readFileSync(path.join(root, ".github", "workflows", name), "utf8");
}

function workflowJob(workflow, name, workflowName) {
  const heading = `  ${name}:\n`;
  const start = workflow.indexOf(heading);
  assert.notEqual(start, -1, `expected ${name} job in ${workflowName}`);
  const remainder = workflow.slice(start + heading.length);
  const nextJobOffset = remainder.search(/^  [a-zA-Z0-9_-]+:\n/m);
  return workflow.slice(
    start,
    nextJobOffset === -1
      ? workflow.length
      : start + heading.length + nextJobOffset,
  );
}

function assertIsolatedSmokeJob(job, description) {
  assert.doesNotMatch(
    job,
    /uses:\s*actions\/checkout@/,
    `${description} must not check out repository source`,
  );
  assert.match(
    job,
    /uses:\s*actions\/download-artifact@v4/,
    `${description} should download built artifacts`,
  );
  assert.match(
    job,
    /Verify source checkout is absent/,
    `${description} should verify that the checkout is absent at runtime`,
  );
  for (const marker of [".git", "Cargo.toml", "cmd"]) {
    assert.match(
      job,
      new RegExp(`for source_path in[^\\n]*${marker.replace(".", "\\.")}`),
      `${description} should reject the ${marker} source marker`,
    );
  }
  assert.match(
    job,
    /archive-smoke\.sh/,
    `${description} should exercise the cargo-dist archive`,
  );
  assert.match(
    job,
    /pypi-wheel-smoke\.sh/,
    `${description} should install and exercise the wheel`,
  );
}

const prWorkflowName = "rust-port-convergence.yml";
const prWorkflow = readWorkflow(prWorkflowName);
const prBuildJob = workflowJob(prWorkflow, "artifact-build", prWorkflowName);
const prSmokeJob = workflowJob(prWorkflow, "artifact-smoke", prWorkflowName);

assert.match(
  prBuildJob,
  /~\/\.cargo\/bin\/dist build/,
  "PR artifact builds should produce cargo-dist archives",
);
assert.match(
  prBuildJob,
  /dist\/wheels\/\*\.whl/,
  "PR artifact builds should produce wheels",
);
assert.match(
  prBuildJob,
  /scripts\/standalone\/archive-smoke\.sh/,
  "PR smoke bundles should include the self-contained harness",
);
assert.match(
  prSmokeJob,
  /needs:\s*artifact-build/,
  "PR smoke jobs should depend on package builds",
);
assertIsolatedSmokeJob(prSmokeJob, "PR artifact smoke job");

const releaseWorkflowName = "devcontainer-release.yml";
const releaseWorkflow = readWorkflow(releaseWorkflowName);
const releaseBuildJob = workflowJob(releaseWorkflow, "build", releaseWorkflowName);
const releaseSmokeJob = workflowJob(
  releaseWorkflow,
  "artifact-smoke",
  releaseWorkflowName,
);
const releaseMuslSmokeJob = workflowJob(
  releaseWorkflow,
  "artifact-smoke-musl",
  releaseWorkflowName,
);
const publishJob = workflowJob(releaseWorkflow, "release", releaseWorkflowName);

assert.doesNotMatch(
  releaseBuildJob,
  /- name:\s*Standalone smoke/,
  "release builds should not smoke artifacts beside the checkout",
);
assert.match(
  releaseBuildJob,
  /name:\s*release-smoke-\$\{\{ matrix\.target \}\}/,
  "release builds should upload named candidate bundles",
);
assertIsolatedSmokeJob(releaseSmokeJob, "release artifact smoke job");
assertIsolatedSmokeJob(releaseMuslSmokeJob, "release musl smoke job");
assert.match(
  releaseMuslSmokeJob,
  /ghcr\.io\/astral-sh\/uv:python3\.12-alpine/,
  "musl artifacts should be exercised inside Alpine",
);
assert.match(
  releaseMuslSmokeJob,
  /--volume "\$bundle:\/bundle:ro"/,
  "the musl container should only receive the smoke bundle",
);
assert.match(
  publishJob,
  /needs:\s*\[prepare, build, artifact-smoke, artifact-smoke-musl\]/,
  "publication should wait for GNU, musl, and macOS artifact smoke jobs",
);
assert.match(
  publishJob,
  /pattern:\s*release-smoke-\*/,
  "publication should download only release candidate bundles",
);
assert.match(
  publishJob,
  /-name '\*\.tar\.gz'.*-o -name '\*\.tar\.gz\.sha256'.*-o -name '\*\.whl'/s,
  "publication should filter harness files out of release assets",
);
