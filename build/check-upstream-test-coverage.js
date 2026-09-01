/*---------------------------------------------------------------------------------------------
 *  Copyright (c) devcontainer-rs contributors.
 *  Licensed under the MIT License.
 *--------------------------------------------------------------------------------------------*/

'use strict';

const fs = require('fs');
const path = require('path');

const repositoryRoot = path.join(__dirname, '..');
const coverageMapPath = path.join(repositoryRoot, 'docs', 'upstream', 'test-coverage-map.json');
const coverageMarkdownPath = path.join(repositoryRoot, 'docs', 'upstream', 'test-coverage-map.md');
const upstreamTestRoot = path.join(repositoryRoot, 'upstream', 'src', 'test');
const compatibilityBaselinePath = path.join(repositoryRoot, 'docs', 'upstream', 'compatibility-baseline.json');

const VALID_STATUSES = new Set(['covered', 'partial', 'missing']);

function fail(message) {
	console.error(message);
	process.exit(1);
}

function walkFiles(rootPath) {
	const stat = fs.statSync(rootPath);
	if (stat.isFile()) {
		return [rootPath];
	}

	return fs.readdirSync(rootPath)
		.sort()
		.flatMap(entry => walkFiles(path.join(rootPath, entry)));
}

function relativePath(absolutePath) {
	return path.relative(repositoryRoot, absolutePath).replace(/\\/g, '/');
}

function loadCoverageMap() {
	if (!fs.existsSync(coverageMapPath)) {
		fail(`Missing coverage map: ${relativePath(coverageMapPath)}`);
	}
	return JSON.parse(fs.readFileSync(coverageMapPath, 'utf8'));
}

function upstreamTests() {
	return walkFiles(upstreamTestRoot)
		.filter(filePath => filePath.endsWith('.test.ts'))
		.map(relativePath)
		.sort();
}

function validateCoverageMap(report) {
	if (!report || typeof report !== 'object') {
		fail('Coverage map must be a JSON object.');
	}

	const baseline = JSON.parse(fs.readFileSync(compatibilityBaselinePath, 'utf8'));
	if (report.upstreamCommit !== baseline.pinnedCommit) {
		fail(`Coverage map upstream commit ${report.upstreamCommit} does not match pinned commit ${baseline.pinnedCommit}.`);
	}

	if (!Array.isArray(report.suites)) {
		fail('Coverage map must contain a suites array.');
	}

	const actualUpstreamTests = upstreamTests();
	const mappedTests = report.suites.map(entry => entry.upstreamTest);
	const duplicates = mappedTests.filter((entry, index) => mappedTests.indexOf(entry) !== index);
	if (duplicates.length) {
		fail(`Coverage map contains duplicate upstream test entries: ${[...new Set(duplicates)].join(', ')}`);
	}

	const missingMappings = actualUpstreamTests.filter(testPath => !mappedTests.includes(testPath));
	if (missingMappings.length) {
		fail(`Coverage map is missing upstream tests: ${missingMappings.join(', ')}`);
	}

	const unexpectedMappings = mappedTests.filter(testPath => !actualUpstreamTests.includes(testPath));
	if (unexpectedMappings.length) {
		fail(`Coverage map references unknown upstream tests: ${unexpectedMappings.join(', ')}`);
	}

	for (const suite of report.suites) {
		if (!VALID_STATUSES.has(suite.status)) {
			fail(`Invalid coverage status for ${suite.upstreamTest}: ${suite.status}`);
		}

		if (!Array.isArray(suite.nativeTests)) {
			fail(`Coverage map entry ${suite.upstreamTest} must contain a nativeTests array.`);
		}

		if (suite.status === 'missing' && suite.nativeTests.length !== 0) {
			fail(`Coverage map entry ${suite.upstreamTest} is marked missing but still lists native tests.`);
		}

		if (suite.status !== 'missing' && suite.nativeTests.length === 0) {
			fail(`Coverage map entry ${suite.upstreamTest} must list native tests for status ${suite.status}.`);
		}

		if (suite.unportedScenarios !== undefined) {
			if (suite.status !== 'partial') {
				fail(`Coverage map entry ${suite.upstreamTest} may only enumerate unported scenarios when marked partial.`);
			}
			if (!Array.isArray(suite.unportedScenarios) || suite.unportedScenarios.length === 0
				|| suite.unportedScenarios.some(scenario => typeof scenario !== 'string' || !scenario.trim())) {
				fail(`Coverage map entry ${suite.upstreamTest} must contain a non-empty unportedScenarios string array.`);
			}
		}

		for (const nativeTest of suite.nativeTests) {
			const absoluteNativePath = path.join(repositoryRoot, nativeTest);
			if (!fs.existsSync(absoluteNativePath)) {
				fail(`Coverage map entry ${suite.upstreamTest} references missing native test path: ${nativeTest}`);
			}
		}
	}
}

function renderMarkdown(report) {
	const counts = {
		covered: report.suites.filter(suite => suite.status === 'covered').length,
		partial: report.suites.filter(suite => suite.status === 'partial').length,
		missing: report.suites.filter(suite => suite.status === 'missing').length,
	};

	const lines = [
		'# Upstream Test Coverage Map',
		'',
		'Machine-readable upstream test coverage inventory for the native Rust CLI.',
		'',
		`- Upstream commit: \`${report.upstreamCommit}\``,
		`- Upstream tests inventoried: \`${report.suites.length}\``,
		`- Covered: \`${counts.covered}\``,
		`- Partial: \`${counts.partial}\``,
		`- Missing: \`${counts.missing}\``,
		'',
		'## Summary',
		'',
		'| Upstream test | Status | Native tests | Notes |',
		'| --- | --- | --- | --- |',
	];

	for (const suite of [...report.suites].sort((left, right) => left.upstreamTest.localeCompare(right.upstreamTest))) {
		const unportedScenarios = suite.unportedScenarios?.length
			? ` Still unported: ${suite.unportedScenarios.join('; ')}.`
			: '';
		lines.push(
			`| \`${suite.upstreamTest}\` | ${suite.status} | ${suite.nativeTests.length ? suite.nativeTests.map(test => `\`${test}\``).join('<br>') : 'none'} | ${suite.notes || ''}${unportedScenarios} |`
		);
	}

	return `${lines.join('\n')}\n`;
}

function main() {
	const report = loadCoverageMap();
	validateCoverageMap(report);

	const generatedMarkdown = renderMarkdown(report);
	if (!fs.existsSync(coverageMarkdownPath)) {
		fail(`Missing coverage markdown: ${relativePath(coverageMarkdownPath)}`);
	}
	if (fs.readFileSync(coverageMarkdownPath, 'utf8') !== generatedMarkdown) {
		fail('Committed upstream test coverage markdown is out of date. Run node build/generate-upstream-test-coverage.js');
	}

	console.log(`[upstream-test-coverage] validated ${report.suites.length} upstream test mappings.`);
}

if (require.main === module) {
	main();
}

module.exports = {
	renderMarkdown,
	upstreamTests,
	validateCoverageMap,
};
