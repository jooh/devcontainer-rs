/*---------------------------------------------------------------------------------------------
 *  Copyright (c) devcontainer-rs contributors.
 *  Licensed under the MIT License.
 *--------------------------------------------------------------------------------------------*/

'use strict';

const cp = require('child_process');
const fs = require('fs');
const path = require('path');

const { generateCommandMatrix } = require('./generate-command-matrix');

const repositoryRoot = path.join(__dirname, '..');
const upstreamCliPath = path.join(
	repositoryRoot,
	'upstream',
	'src',
	'spec-node',
	'devContainersSpecCLI.ts',
);
const parityInventoryPath = path.join(
	repositoryRoot,
	'docs',
	'upstream',
	'parity-inventory.json',
);
const outputPath = path.join(
	repositoryRoot,
	'cmd',
	'devcontainer',
	'src',
	'cli_metadata.json',
);

const nativeOptionsByCommand = {
	up: [
		{
			name: 'pull-always',
			aliases: [],
			description: 'Always pull images before creating the dev container. Native extension.  [boolean]',
		},
	],
	build: [
		{
			name: 'build-no-cache',
			aliases: [],
			description: 'Build without using cached layers. Native extension.  [boolean]',
		},
	],
	'set-up': [
		{
			name: 'workspace-folder',
			aliases: [],
			description: 'Workspace folder used to resolve the devcontainer configuration. Native extension.  [string]',
		},
		{
			name: 'docker-compose-path',
			aliases: [],
			description: 'Docker Compose CLI path. Native extension.  [string]',
		},
	],
	exec: [
		{
			name: 'secrets-file',
			aliases: [],
			description: 'Path to a JSON file containing secret environment variables. Native extension.  [string]',
		},
	],
	'features test': [
		{
			name: 'docker-path',
			aliases: [],
			description: 'Container engine CLI path. Native extension.  [string]',
		},
	],
	'features publish': [
		{
			name: 'output-dir',
			aliases: [],
			description: 'Directory for the generated local OCI layout. Native extension.  [string]',
		},
	],
	'templates metadata': [
		{
			name: 'workspace-folder',
			aliases: [],
			description: 'Workspace folder used to resolve local OCI layouts. Native extension.  [string]',
		},
	],
	'templates publish': [
		{
			name: 'output-dir',
			aliases: [],
			description: 'Directory for the generated local OCI layout. Native extension.  [string]',
		},
	],
};

// Native collection handlers retain positional forms that are not declared by
// the pinned upstream yargs command definitions. Keep them in validation
// metadata without changing the upstream-rendered help text.
const nativePositionalsByCommand = {
	'templates apply': [
		{
			name: 'target',
			description: 'Local template folder to apply. Native extension.  [string]',
		},
	],
	'features generate-docs': [
		{
			name: 'target',
			description: 'Feature collection folder. Native extension.  [string]',
		},
	],
	'templates generate-docs': [
		{
			name: 'target',
			description: 'Template collection folder. Native extension.  [string]',
		},
	],
};

// Hidden upstream options do not appear in rendered help, so retain the type
// and alias details needed to parse their values without making them visible.
const hiddenOptionsByCommand = {
	'features test': [
		{
			name: 'projectFolder',
			aliases: [],
			description: 'Project folder accepted by the native feature test runner.  [string]',
			visible: false,
		},
	],
	upgrade: [
		{
			name: 'feature',
			aliases: ['f'],
			description: 'Feature identifier to upgrade.  [string]',
			visible: false,
		},
		{
			name: 'target-version',
			aliases: ['v'],
			description: 'Feature version requirement to apply.  [string]',
			visible: false,
		},
	],
};

function readJson(filePath) {
	return JSON.parse(fs.readFileSync(filePath, 'utf8'));
}

function runUpstreamHelp(args) {
	return cp.execFileSync(
		'node',
		[
			'-r',
			'ts-node/register/transpile-only',
			upstreamCliPath,
			...args,
			'--help',
		],
		{
			cwd: repositoryRoot,
			encoding: 'utf8',
			env: {
				...process.env,
				TS_NODE_COMPILER_OPTIONS: '{"moduleResolution":"NodeNext"}',
			},
			stdio: ['ignore', 'pipe', 'pipe'],
		},
	);
}

function normalizeHelpText(rawText) {
	const lines = rawText.replace(/\r/g, '').trimEnd().split('\n');
	while (lines.length && !lines[lines.length - 1].trim()) {
		lines.pop();
	}
	if (lines.length && lines[lines.length - 1].startsWith('devcontainer@')) {
		lines.pop();
		while (lines.length && !lines[lines.length - 1].trim()) {
			lines.pop();
		}
	}
	return lines;
}

function splitHelpColumns(line) {
	const trimmed = line.trimStart();
	const parts = trimmed.split(/\s{2,}/);
	if (parts.length < 2) {
		return null;
	}
	return {
		label: parts[0],
		description: parts.slice(1).join('  '),
	};
}

function parseOptionLine(line) {
	const columns = splitHelpColumns(line);
	if (!columns || !columns.label.includes('--')) {
		return null;
	}
	const aliases = [...columns.label.matchAll(/(?:^|,\s*)-([A-Za-z0-9])(?:,|$|\s)/g)].map(
		match => match[1],
	);
	const longNames = [...columns.label.matchAll(/--([A-Za-z0-9][A-Za-z0-9-]*)/g)].map(
		match => match[1],
	);
	if (!longNames.length) {
		return null;
	}
	return {
		name: longNames[longNames.length - 1],
		aliases,
		description: columns.description,
	};
}

function parsePositionalLine(line) {
	// Yargs renders long positional descriptions on continuation lines, leaving
	// the first line as just "  <name>". Only consider entries at the section's
	// two-space indentation so those continuation lines are not mistaken for
	// additional positionals.
	if (!/^ {2}\S/.test(line)) {
		return null;
	}
	const columns = splitHelpColumns(line);
	const label = columns ? columns.label : line.trim();
	const name = label.split(/\s+/)[0];
	if (!name || name.startsWith('-') || name.startsWith('[')) {
		return null;
	}
	return {
		name,
		description: columns ? columns.description : '',
	};
}

function parseDisplayedEntries(lines) {
	const renderedLines = [];
	const displayedOptions = [];
	const displayedPositionals = [];
	let section = null;

	for (const line of lines) {
		if (/^(Commands|Positionals|Options):$/.test(line.trim())) {
			section = line.trim().slice(0, -1);
			renderedLines.push({
				text: line,
				optionNames: [],
				positionalNames: [],
			});
			continue;
		}

		const option = section === 'Options' ? parseOptionLine(line) : null;
		if (option) {
			displayedOptions.push(option);
		}
		const positional = section === 'Positionals' ? parsePositionalLine(line) : null;
		if (positional) {
			displayedPositionals.push(positional);
		}

		renderedLines.push({
			text: line,
			optionNames: option ? [option.name] : [],
			positionalNames: positional ? [positional.name] : [],
		});
	}

	return {
		lines: renderedLines,
		displayedOptions,
		displayedPositionals,
	};
}

function mergeUsagePositionals(commandPath, lines, displayedPositionals) {
	const usagePrefix = `devcontainer ${commandPath}`;
	const usageLine = lines[0] || '';
	if (!usageLine.startsWith(usagePrefix)) {
		return displayedPositionals;
	}

	const byName = new Map(displayedPositionals.map(positional => [positional.name, positional]));
	const merged = [];
	for (const match of usageLine.slice(usagePrefix.length).matchAll(/<([^>]+)>|\[([^\]]+)\]/g)) {
		const name = (match[1] || match[2]).replace(/\.\.$/, '');
		if (!byName.has(name)) {
			byName.set(name, { name, description: '' });
		}
		merged.push(byName.get(name));
	}
	for (const positional of displayedPositionals) {
		if (!merged.some(candidate => candidate.name === positional.name)) {
			merged.push(positional);
		}
	}
	return merged;
}

function nativeOptionsForCommand(commandPath) {
	return nativeOptionsByCommand[commandPath] || [];
}

function nativePositionalsForCommand(commandPath) {
	return nativePositionalsByCommand[commandPath] || [];
}

function hiddenOptionsForCommand(commandPath) {
	return hiddenOptionsByCommand[commandPath] || [];
}

function nativeOptionLines(options) {
	return options.map(option => ({
		text: `  --${option.name.padEnd(32)}${option.description}`,
		optionNames: [option.name],
		positionalNames: [],
	}));
}

function mergeOptions(allOptionNames, displayedOptions) {
	const displayedByName = new Map(displayedOptions.map(option => [option.name, option]));
	const merged = [];
	const seenNames = new Set();
	for (const name of allOptionNames) {
		if (seenNames.has(name)) {
			continue;
		}
		seenNames.add(name);
		const displayed = displayedByName.get(name);
		merged.push({
			name,
			aliases: displayed ? displayed.aliases : [],
			description: displayed ? displayed.description : null,
			visible: Boolean(displayed) && displayed.visible !== false,
		});
	}

	for (const displayed of displayedOptions) {
		if (seenNames.has(displayed.name)) {
			continue;
		}
		seenNames.add(displayed.name);
		merged.push({
			name: displayed.name,
			aliases: displayed.aliases,
			description: displayed.description,
			visible: true,
		});
	}

	return merged;
}

function unsupportedOptionsForCommand(parityInventory, commandPath) {
	const command = parityInventory.commands.find(entry => entry.path === commandPath);
	if (!command) {
		return [];
	}
	return command.options
		.filter(option => !option.sourceReferenced)
		.map(option => option.name)
		.sort();
}

function groupChildren(matrix, commandPath) {
	return matrix.commands
		.filter(command => command.group === commandPath)
		.map(command => command.path.split(' ').slice(-1)[0])
		.sort();
}

function generateCliMetadata() {
	const matrix = generateCommandMatrix();
	const parityInventory = readJson(parityInventoryPath);

	const rootLines = normalizeHelpText(runUpstreamHelp([]));
	const root = parseDisplayedEntries(rootLines);

	const commands = matrix.commands.map(command => {
		const commandLines = normalizeHelpText(
			runUpstreamHelp(command.path.split(' ')),
		);
		const parsed = parseDisplayedEntries(commandLines);
		const nativeOptions = nativeOptionsForCommand(command.path);
		const nativePositionals = nativePositionalsForCommand(command.path);
		const hiddenOptions = hiddenOptionsForCommand(command.path);
		const positionals = mergeUsagePositionals(
			command.path,
			commandLines,
			[...parsed.displayedPositionals, ...nativePositionals],
		);
		return {
			path: command.path,
			group: command.group,
			tokenPath: command.path.split(' '),
			description: command.description,
			subcommands: groupChildren(matrix, command.path),
			lines: [...parsed.lines, ...nativeOptionLines(nativeOptions)],
			options: mergeOptions(
				[
					...command.options,
					...nativeOptions.map(option => option.name),
					...hiddenOptions.map(option => option.name),
				],
				[...parsed.displayedOptions, ...nativeOptions, ...hiddenOptions],
			),
			positionals,
			unsupportedOptions: unsupportedOptionsForCommand(
				parityInventory,
				command.path,
			),
			unsupportedPositionals: [],
		};
	});

	return {
		upstreamCommit: matrix.upstreamCommit,
		sourcePath: matrix.sourcePath,
		root: {
			lines: root.lines,
			options: root.displayedOptions.map(option => ({
				name: option.name,
				aliases: option.aliases,
				description: option.description,
				visible: true,
			})),
			subcommands: matrix.topLevel,
		},
		commands,
	};
}

function writeMetadata(metadata) {
	fs.writeFileSync(outputPath, `${JSON.stringify(metadata, null, '\t')}\n`);
}

function compareToCommitted(metadata) {
	if (!fs.existsSync(outputPath)) {
		throw new Error(
			`Missing committed CLI metadata: ${path.relative(repositoryRoot, outputPath)}`,
		);
	}
	const committed = fs.readFileSync(outputPath, 'utf8');
	const generated = `${JSON.stringify(metadata, null, '\t')}\n`;
	return committed === generated;
}

if (require.main === module) {
	const metadata = generateCliMetadata();
	if (process.argv.includes('--check')) {
		if (!compareToCommitted(metadata)) {
			console.error(
				'Committed CLI metadata is out of date. Run node build/generate-cli-metadata.js',
			);
			process.exit(1);
		}
		console.log('[cli-metadata] committed metadata matches pinned upstream help.');
	} else {
		writeMetadata(metadata);
		console.log(`[cli-metadata] wrote ${path.relative(repositoryRoot, outputPath)}`);
	}
}

module.exports = {
	generateCliMetadata,
	writeMetadata,
};
