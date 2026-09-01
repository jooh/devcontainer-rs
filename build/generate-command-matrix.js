/*---------------------------------------------------------------------------------------------
 *  Copyright (c) devcontainer-rs contributors.
 *  Licensed under the MIT License.
 *--------------------------------------------------------------------------------------------*/

'use strict';

const cp = require('child_process');
const fs = require('fs');
const path = require('path');

const repositoryRoot = path.join(__dirname, '..');
const upstreamCliPath = path.join(repositoryRoot, 'upstream', 'src', 'spec-node', 'devContainersSpecCLI.ts');
const outputPath = path.join(repositoryRoot, 'docs', 'upstream', 'command-matrix.json');

function runGit(args) {
	return cp.execFileSync('git', args, {
		cwd: repositoryRoot,
		encoding: 'utf8',
		stdio: ['ignore', 'pipe', 'pipe'],
	}).trim();
}

function readFile(filePath) {
	return fs.readFileSync(filePath, 'utf8');
}

function extractFunctionBlock(source, functionName) {
	const signature = `function ${functionName}(y: Argv) {`;
	const start = source.indexOf(signature);
	if (start === -1) {
		return '';
	}

	let depth = 0;
	let end = start;
	for (let index = start; index < source.length; index += 1) {
		const char = source[index];
		if (char === '{') {
			depth += 1;
		} else if (char === '}') {
			depth -= 1;
			if (depth === 0) {
				end = index + 1;
				break;
			}
		}
	}

	return source.slice(start, end);
}

function parseImports(source) {
	const importMap = new Map();
	const importRegex = /import\s+\{\s*([^}]+)\s*\}\s+from\s+'([^']+)';/g;
	let match;
	while ((match = importRegex.exec(source)) !== null) {
		const names = match[1].split(',').map(name => name.trim()).filter(Boolean);
		const resolvedPath = path.resolve(path.dirname(upstreamCliPath), `${match[2]}.ts`);
		for (const name of names) {
			importMap.set(name, resolvedPath);
		}
	}
	return importMap;
}

function parseOptionNames(functionName, sourceCache, importMap) {
	const sourcePath = importMap.get(functionName) ?? upstreamCliPath;
	if (!sourceCache.has(sourcePath)) {
		sourceCache.set(sourcePath, readFile(sourcePath));
	}
	const source = sourceCache.get(sourcePath);
	const block = extractFunctionBlock(source, functionName);
	if (!block) {
		return [];
	}

	const names = new Set();
	const objectOptionRegex = /'([^']+)':\s*\{/g;
	let match;
	while ((match = objectOptionRegex.exec(block)) !== null) {
		names.add(match[1]);
	}

	const chainedOptionRegex = /\.option\('([^']+)'/g;
	while ((match = chainedOptionRegex.exec(block)) !== null) {
		names.add(match[1]);
	}

	return Array.from(names).sort();
}

function parseCommandBlock(source) {
	const start = source.indexOf("y.command('up'");
	const end = source.indexOf('y.epilog(');
	if (start === -1 || end === -1) {
		throw new Error(`Unable to locate CLI command block in ${path.relative(repositoryRoot, upstreamCliPath)}.`);
	}
	return source.slice(start, end);
}

function parseGlobalOptionNames(source) {
	const start = source.indexOf('const y = yargs(');
	const end = source.indexOf("y.command('up'");
	if (start === -1 || end === -1 || end <= start) {
		throw new Error(`Unable to locate CLI global options in ${path.relative(repositoryRoot, upstreamCliPath)}.`);
	}
	const names = new Set();
	const optionRegex = /\.option\('([^']+)'/g;
	let match;
	const block = source.slice(start, end);
	while ((match = optionRegex.exec(block)) !== null) {
		names.add(match[1]);
	}
	return Array.from(names).sort();
}

function parseCommands(commandBlock) {
	const lines = commandBlock.split('\n');
	const commands = [];
	let currentGroup = null;

	for (const line of lines) {
		const trimmed = line.trim();
		if (!trimmed.startsWith('y.command(')) {
			if (currentGroup && trimmed === '});') {
				currentGroup = null;
			}
			continue;
		}

		if (trimmed.includes("restArgs ? ['exec', '*']")) {
			commands.push({
				group: null,
				path: 'exec',
				description: 'Execute a command on a running dev container',
				optionsBuilder: 'execOptions',
			});
			continue;
		}

		const groupMatch = trimmed.match(/^y\.command\('([^']+)'\s*,\s*'((?:\\'|[^'])+)'\s*,\s*\(y: Argv\) => \{$/);
		if (groupMatch) {
			currentGroup = groupMatch[1];
			commands.push({
				group: null,
				path: groupMatch[1],
				description: groupMatch[2],
				optionsBuilder: null,
			});
			continue;
		}

		const commandMatch = trimmed.match(/^y\.command\('([^']+)'\s*,\s*'((?:\\'|[^'])+)'\s*,\s*([A-Za-z0-9_]+),\s*[A-Za-z0-9_]+\);$/);
		if (commandMatch) {
			const relativePath = commandMatch[1].split(' ')[0];
			commands.push({
				group: currentGroup,
				path: currentGroup ? `${currentGroup} ${relativePath}` : relativePath,
				description: commandMatch[2],
				optionsBuilder: commandMatch[3],
			});
		}
	}

	return commands;
}

function generateCommandMatrix() {
	const source = readFile(upstreamCliPath);
	const importMap = parseImports(source);
	const sourceCache = new Map([[upstreamCliPath, source]]);
	const commands = parseCommands(parseCommandBlock(source))
		.map(command => ({
			...command,
			options: command.optionsBuilder ? parseOptionNames(command.optionsBuilder, sourceCache, importMap) : [],
		}));

	const topLevel = commands.filter(command => !command.group).map(command => command.path);
	const subcommands = commands.filter(command => command.group);

	return {
		upstreamCommit: runGit(['rev-parse', 'HEAD:upstream']),
		sourcePath: path.relative(repositoryRoot, upstreamCliPath),
		globalOptions: parseGlobalOptionNames(source),
		topLevel,
		commands,
		allCommandPaths: commands.map(command => command.path),
		subcommandsByGroup: subcommands.reduce((accumulator, command) => {
			const group = command.group;
			accumulator[group] = accumulator[group] || [];
			accumulator[group].push(command.path);
			return accumulator;
		}, {}),
	};
}

function writeMatrix(matrix) {
	fs.writeFileSync(outputPath, `${JSON.stringify(matrix, null, '\t')}\n`);
}

function compareToCommitted(matrix) {
	if (!fs.existsSync(outputPath)) {
		throw new Error(`Missing committed command matrix: ${path.relative(repositoryRoot, outputPath)}`);
	}
	const committed = readFile(outputPath);
	const generated = `${JSON.stringify(matrix, null, '\t')}\n`;
	return committed === generated;
}

if (require.main === module) {
	const matrix = generateCommandMatrix();
	if (process.argv.includes('--check')) {
		if (!compareToCommitted(matrix)) {
			console.error('Committed command matrix is out of date. Run node build/generate-command-matrix.js');
			process.exit(1);
		}
		console.log('[command-matrix] committed matrix matches pinned upstream sources.');
	} else {
		writeMatrix(matrix);
		console.log(`[command-matrix] wrote ${path.relative(repositoryRoot, outputPath)}`);
	}
}

module.exports = {
	generateCommandMatrix,
	writeMatrix,
};
