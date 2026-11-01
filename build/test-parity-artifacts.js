/*---------------------------------------------------------------------------------------------
 *  Copyright (c) devcontainer-rs contributors.
 *  Licensed under the MIT License.
 *--------------------------------------------------------------------------------------------*/

'use strict';

const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const test = require('node:test');

const { buildInventory } = require('./generate-parity-inventory');
const { generateCommandMatrix } = require('./generate-command-matrix');

const repositoryRoot = path.join(__dirname, '..');
const coverageMapPath = path.join(repositoryRoot, 'docs', 'upstream', 'test-coverage-map.json');

test('shared upstream Docker argument definitions appear in every supported command', () => {
	const matrix = generateCommandMatrix();
	for (const command of ['up', 'set-up', 'build', 'run-user-commands', 'read-configuration', 'exec', 'upgrade']) {
		assert.ok(matrix.commands.find(entry => entry.path === command)?.options.includes('docker-arg'), command);
	}
});

test('generated CLI metadata is not accepted as native option evidence', () => {
	const inventory = buildInventory();
	const evidence = [
		...inventory.globalOptions.flatMap(option => option.evidence),
		...inventory.commands.flatMap(command => command.options.flatMap(option => option.evidence)),
	];

	assert.ok(!evidence.includes('cmd/devcontainer/src/cli_metadata.json'));
});

test('partial OCI and Feature configuration mappings enumerate unported scenarios', () => {
	const report = JSON.parse(fs.readFileSync(coverageMapPath, 'utf8'));
	for (const upstreamTest of [
		'upstream/src/test/container-features/containerFeaturesOCI.test.ts',
		'upstream/src/test/container-features/generateFeaturesConfig.test.ts',
	]) {
		const suite = report.suites.find(candidate => candidate.upstreamTest === upstreamTest);
		assert.ok(suite, `missing coverage entry for ${upstreamTest}`);
		assert.equal(suite.status, 'partial');
		assert.ok(Array.isArray(suite.unportedScenarios));
		assert.ok(suite.unportedScenarios.length > 0);
		assert.ok(suite.unportedScenarios.every(scenario => typeof scenario === 'string' && scenario.length > 0));
	}
});
