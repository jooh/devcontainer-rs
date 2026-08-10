/*---------------------------------------------------------------------------------------------
 *  Copyright (c) devcontainer-rs contributors.
 *  Licensed under the MIT License.
 *--------------------------------------------------------------------------------------------*/

'use strict';

const assert = require('assert');
const fs = require('fs');
const path = require('path');

const repositoryRoot = path.join(__dirname, '..');
const devcontainerRoot = path.join(repositoryRoot, '.devcontainer');
const configPath = path.join(devcontainerRoot, 'devcontainer.json');
const containerfilePath = path.join(devcontainerRoot, 'Containerfile');
const legacyDockerfilePath = path.join(devcontainerRoot, 'Dockerfile');

function stripJsonComments(text) {
	let result = '';
	let inString = false;
	let escaped = false;
	let inLineComment = false;
	let inBlockComment = false;

	for (let index = 0; index < text.length; index += 1) {
		const current = text[index];
		const next = text[index + 1];

		if (inLineComment) {
			if (current === '\n') {
				inLineComment = false;
				result += current;
			}
			continue;
		}

		if (inBlockComment) {
			if (current === '*' && next === '/') {
				inBlockComment = false;
				index += 1;
			}
			continue;
		}

		if (inString) {
			result += current;
			if (escaped) {
				escaped = false;
			} else if (current === '\\') {
				escaped = true;
			} else if (current === '"') {
				inString = false;
			}
			continue;
		}

		if (current === '"') {
			inString = true;
			result += current;
			continue;
		}

		if (current === '/' && next === '/') {
			inLineComment = true;
			index += 1;
			continue;
		}

		if (current === '/' && next === '*') {
			inBlockComment = true;
			index += 1;
			continue;
		}

		result += current;
	}

	return result;
}

function stripTrailingCommas(text) {
	return text.replace(/,\s*([}\]])/g, '$1');
}

function parseJsonc(text) {
	return JSON.parse(stripTrailingCommas(stripJsonComments(text)));
}

function main() {
	assert(fs.existsSync(configPath), '.devcontainer/devcontainer.json must exist');
	assert(fs.existsSync(containerfilePath), '.devcontainer/Containerfile must exist');
	assert(!fs.existsSync(legacyDockerfilePath), '.devcontainer/Dockerfile must not exist');

	const config = parseJsonc(fs.readFileSync(configPath, 'utf8'));
	assert.equal(config.build?.dockerfile, 'Containerfile', 'devcontainer must build from .devcontainer/Containerfile');
	assert.equal(config.build?.context, '..', 'devcontainer build context must remain repository root');
	assert.equal(config.remoteUser, 'dev', 'devcontainer should use the repo-owned dev user');
	assert.equal(config.updateRemoteUserUID, true, 'devcontainer should keep host UID/GID alignment enabled');
	assert.equal(
		config.postCreateCommand,
		'git config --global --add safe.directory ${containerWorkspaceFolder} && git submodule update --init --recursive && COREPACK_ENABLE_PROJECT_SPEC=0 corepack yarn --cwd upstream install --frozen-lockfile --modules-folder ../node_modules',
		'postCreateCommand should initialize submodules and install pinned upstream tooling dependencies',
	);
	assert(!('features' in config), 'devcontainer should not depend on external devcontainer features');
	assert(!('customizations' in config), 'devcontainer should not carry editor-specific customizations');

	const containerfile = fs.readFileSync(containerfilePath, 'utf8');
	assert(/FROM\s+docker\.io\/library\/rust:1-bookworm/i.test(containerfile), 'Containerfile should start from the official Rust Bookworm image');
	assert(/ARG\s+NODE_VERSION=20\./.test(containerfile), 'Containerfile should provision Node 20.x for repo checks');
	assert(
		/useradd\s+--create-home\s+--shell\s+\/bin\/bash\s+--uid\s+["$A-Za-z0-9{}_:-]+\s+--gid\s+["$A-Za-z0-9{}_:-]+\s+["$A-Za-z0-9{}_:-]+/.test(containerfile),
		'Containerfile should create the dev user',
	);

	console.log('[devcontainer-config] repo-owned devcontainer definition looks current.');
}

main();
