'use strict';

const fs = require('node:fs');
const path = require('node:path');
const crypto = require('node:crypto');
const root = path.resolve(__dirname, '..');
const destination = path.join(root, 'docs/src/site-brand');
const pinPath = path.join(root, 'docs/theme/site-brand.json');
const sourcePath = 'content/extra/site-brand';
const files = ['site-brand.css', 'LICENSE', ...[
    '300-normal', '400-normal', '700-normal', '400-italic',
].map(style => `fonts/source-sans-pro-latin-${style}.woff2`)];

/**
 * Sync an immutable website revision or verify the local snapshot offline.
 * @returns {Promise<void>}
 */
async function main() {
    const revision = process.argv[2];
    if (revision === '--check') {
        const pin = JSON.parse(fs.readFileSync(pinPath, 'utf8'));
        for (const file of files) {
            const hash = crypto.createHash('sha256').update(fs.readFileSync(path.join(destination, file))).digest('hex');
            if (pin.files[file] !== hash) throw new Error(`Shared styling drift: ${file}. Run the documented sync command.`);
        }
        console.log(`Shared styling matches website revision ${pin.revision}.`);
        return;
    }
    if (!/^[a-f0-9]{40}$/.test(revision || '')) throw new Error('Usage: node build/sync-site-brand.js <full-website-commit-sha> | --check');
    const downloads = await Promise.all(files.map(async file => {
        const url = `https://raw.githubusercontent.com/jooh/jooh.github.io/${revision}/${sourcePath}/${file}`;
        const response = await fetch(url);
        if (!response.ok) throw new Error(`Cannot fetch ${file}: HTTP ${response.status}`);
        return { file, data: Buffer.from(await response.arrayBuffer()) };
    }));
    const hashes = {};
    for (const {file, data} of downloads) {
        const target = path.join(destination, file);
        fs.mkdirSync(path.dirname(target), {recursive: true});
        fs.writeFileSync(target, data);
        hashes[file] = crypto.createHash('sha256').update(data).digest('hex');
    }
    fs.writeFileSync(pinPath, JSON.stringify({repository: 'jooh/jooh.github.io', revision, path: sourcePath, files: hashes}, null, 2) + '\n');
}

main().catch(error => { console.error(error.message); process.exitCode = 1; });
