.PHONY: tests \
	rust-fmt \
	rust-clippy \
	rust-check \
	rust-doc \
	rust-tests \
	cargo-deny-check \
	build-release \
	real-engine-lifecycle-smoke \
	real-engine-lifecycle-smoke-docker \
	real-engine-lifecycle-smoke-podman \
	standalone-artifact-smoke \
	pypi-wheel-smoke \
	native-only-startup-contract \
	acceptance-fixtures-check \
	check-upstream-submodule \
	command-matrix-drift-check \
	check-cli-reference \
	schema-drift-check \
	parity-harness \
	no-node-runtime \
	npm-wrapper-check \
	npm-publish-script-check \
	npm-package-smoke \
	tap-check \
	homebrew-distribution-check \
	npm-publish-workflow-check \
	check-parity-inventory \
	check-cli-metadata \
	check-compatibility-dashboard \
	check-upstream-test-coverage \
	check-devcontainer-config \
	upstream-compatibility

RUST_MANIFEST := cmd/devcontainer/Cargo.toml
RELEASE_BINARY := ./cmd/devcontainer/target/release/devcontainer

tests: rust-fmt rust-clippy rust-check rust-doc rust-tests cargo-deny-check build-release standalone-artifact-smoke pypi-wheel-smoke native-only-startup-contract acceptance-fixtures-check check-upstream-submodule command-matrix-drift-check check-cli-reference schema-drift-check parity-harness no-node-runtime npm-wrapper-check npm-publish-script-check npm-package-smoke tap-check homebrew-distribution-check npm-publish-workflow-check check-parity-inventory check-cli-metadata check-compatibility-dashboard check-upstream-test-coverage check-devcontainer-config upstream-compatibility

rust-fmt:
	cargo fmt --manifest-path $(RUST_MANIFEST) --all -- --check

rust-clippy:
	cargo clippy --manifest-path $(RUST_MANIFEST) --all-targets --all-features -- -D warnings

rust-check:
	cargo check --manifest-path $(RUST_MANIFEST) --all-targets --all-features

rust-doc:
	cargo doc --manifest-path $(RUST_MANIFEST) --no-deps --document-private-items

rust-tests:
	cargo test --manifest-path $(RUST_MANIFEST) --locked

cargo-deny-check:
	cargo deny --manifest-path $(RUST_MANIFEST) check -A license-not-encountered

build-release:
	cargo build --release --manifest-path $(RUST_MANIFEST)

real-engine-lifecycle-smoke: real-engine-lifecycle-smoke-docker real-engine-lifecycle-smoke-podman

real-engine-lifecycle-smoke-docker: build-release
	./scripts/standalone/real-engine-smoke.sh $(RELEASE_BINARY)

real-engine-lifecycle-smoke-podman: build-release
	./scripts/standalone/real-engine-smoke.sh $(RELEASE_BINARY) --docker-path podman --docker-compose-path podman-compose

standalone-artifact-smoke: build-release
	./scripts/standalone/smoke.sh $(RELEASE_BINARY)

pypi-wheel-smoke:
	./scripts/standalone/pypi-wheel-smoke.sh

native-only-startup-contract:
	node build/check-native-only.js

acceptance-fixtures-check:
	node build/check-acceptance-fixtures.js

check-upstream-submodule:
	node build/check-upstream-submodule.js

command-matrix-drift-check:
	node build/generate-command-matrix.js --check

check-cli-reference:
	node build/generate-cli-reference.js --check

schema-drift-check:
	node build/check-spec-drift.js

parity-harness:
	node build/check-parity-harness.js

no-node-runtime:
	node build/check-no-node-runtime.js

npm-wrapper-check:
	node --test build/test-npm-wrapper.js

npm-publish-script-check:
	node --test build/test-publish-npm-packages.js

npm-package-smoke:
	node --test build/test-npm-package-smoke.js
	node build/check-npm-packages.js

tap-check:
	npm --prefix tap test

homebrew-distribution-check:
	node build/check-homebrew-distribution.js

npm-publish-workflow-check:
	node build/check-npm-publish-workflow.js

check-parity-inventory:
	node build/generate-parity-inventory.js --check

check-cli-metadata:
	node build/generate-cli-metadata.js --check

check-compatibility-dashboard:
	node build/generate-compatibility-dashboard.js --check

check-upstream-test-coverage:
	node build/check-upstream-test-coverage.js

check-devcontainer-config:
	node build/check-devcontainer-config.js

upstream-compatibility:
	node build/check-upstream-compatibility.js
