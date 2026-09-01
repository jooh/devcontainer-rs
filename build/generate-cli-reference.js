/*---------------------------------------------------------------------------------------------
 *  Copyright (c) devcontainer-rs contributors.
 *  Licensed under the MIT License.
 *--------------------------------------------------------------------------------------------*/

'use strict';

const fs = require('fs');
const path = require('path');

const { generateCommandMatrix } = require('./generate-command-matrix');

const repositoryRoot = path.join(__dirname, '..');
const outputPath = path.join(repositoryRoot, 'docs', 'upstream', 'command-reference.md');

function renderOptions(options) {
	if (!options.length) {
		return '- None';
	}
	return options.map(option => `- \`--${option}\``).join('\n');
}

function renderCommand(command) {
	return [
		`### \`${command.path}\``,
		'',
		command.description,
		'',
		'Options:',
		renderOptions(command.options),
		'',
	].join('\n');
}

function renderReference(matrix) {
	const topLevelCommands = matrix.commands.filter(command => command.group === null);
	const groupedSubcommands = Object.entries(matrix.subcommandsByGroup);

	return [
		'# Upstream CLI Command Reference',
		'',
		'Generated from the pinned upstream CLI command matrix. This is a compatibility baseline, not a native behavior reference.',
		'',
		`- Upstream commit: \`${matrix.upstreamCommit}\``,
		`- Source: \`${matrix.sourcePath}\``,
		'',
		'## Global Options',
		'',
		renderOptions(matrix.globalOptions || []),
		'',
		'## Top-Level Commands',
		'',
		'| Command | Description |',
		'| --- | --- |',
		...topLevelCommands.map(command => `| \`${command.path}\` | ${command.description} |`),
		'',
		'## Detailed Reference',
		'',
		...topLevelCommands.map(renderCommand),
		...groupedSubcommands.flatMap(([group, commandPaths]) => {
			const commands = matrix.commands.filter(command => command.group === group);
			return [
				`## \`${group}\` Subcommands`,
				'',
				...commands.map(renderCommand),
			];
		}),
	].join('\n');
}

function writeReference(text) {
	fs.mkdirSync(path.dirname(outputPath), { recursive: true });
	fs.writeFileSync(outputPath, `${text}\n`);
}

function compareToCommitted(text) {
	if (!fs.existsSync(outputPath)) {
		throw new Error(`Missing committed CLI reference: ${path.relative(repositoryRoot, outputPath)}`);
	}

	return fs.readFileSync(outputPath, 'utf8') === `${text}\n`;
}

if (require.main === module) {
	const matrix = generateCommandMatrix();
	const reference = renderReference(matrix);
	if (process.argv.includes('--check')) {
		if (!compareToCommitted(reference)) {
			console.error('Committed upstream CLI reference is out of date. Run node build/generate-cli-reference.js');
			process.exit(1);
		}
		console.log('[cli-reference] committed upstream reference matches pinned upstream command matrix.');
	} else {
		writeReference(reference);
		console.log(`[cli-reference] wrote ${path.relative(repositoryRoot, outputPath)}`);
	}
}

module.exports = {
	renderReference,
	writeReference,
};
