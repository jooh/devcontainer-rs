'use strict';

const { execFileSync } = require('node:child_process');
const fs = require('node:fs');
const path = require('node:path');

const root = path.resolve(__dirname, '..');
const destination = path.join(root, 'docs/src/cli.md');

/**
 * Render the native binary's help for every declared command.
 * @param {string} binaryPath
 * @returns {string}
 */
function renderReference(binaryPath) {
	const metadata = JSON.parse(fs.readFileSync(path.join(root, 'cmd/devcontainer/src/cli_metadata.json'), 'utf8'));
	const commands = [[], ...metadata.commands.map(command => command.tokenPath)];
	const sections = [
		'# CLI reference',
		'',
		"Generated from the native binary's `--help` output.",
		'',
		'This describes the current development version of the CLI. An installed',
		'release may differ; use its `--help` for the options available there.',
		'',
	];
	for (const tokens of commands) {
		const command = ['devcontainer', ...tokens].join(' ');
		const help = execFileSync(binaryPath, [...tokens, '--help'], { encoding: 'utf8' }).trimEnd();
		sections.push(`## ${command}`, '', '```text', help, '```', '');
	}
	return sections.join('\n');
}

/**
 * Write the reference or check that it matches the compiled CLI.
 * @returns {number}
 */
function main() {
	const reference = renderReference(path.join(root, 'cmd/devcontainer/target/debug/devcontainer'));
	if (process.argv.includes('--check')) {
		if (!fs.existsSync(destination) || fs.readFileSync(destination, 'utf8') !== reference) {
			console.error('Native CLI reference is out of date. Run make docs-reference.');
			return 1;
		}
		console.log("Native CLI reference matches the binary's help output.");
	} else {
		fs.writeFileSync(destination, reference);
	}
	return 0;
}

process.exitCode = main();
