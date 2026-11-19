import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const REPO = 'z55183980-gif/starnexus-keytool';
const artifacts = {
  'windows-x86_64-nsis': 'StarNexus-Key-Tool_Windows_x64-setup.exe',
  'windows-x86_64-msi': 'StarNexus-Key-Tool_Windows_x64.msi',
  'darwin-x86_64': 'StarNexus-Key-Tool_macOS.app.tar.gz',
  'darwin-aarch64': 'StarNexus-Key-Tool_macOS.app.tar.gz',
};

/** Emit a single manifest only after both build jobs have produced signed packages. */
export function buildManifest(version, directory, pubDate = new Date().toISOString()) {
  if (!/^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/.test(version)) throw Error('Invalid release version');
  const platforms = {};
  for (const [platform, filename] of Object.entries(artifacts)) {
    const artifact = path.join(directory, filename);
    if (!fs.statSync(artifact).isFile() || fs.statSync(artifact).size === 0) throw Error(`Empty update artifact: ${filename}`);
    const signature = fs.readFileSync(`${artifact}.sig`, 'utf8').trim();
    if (!signature || !Buffer.from(signature, 'base64').toString('utf8').startsWith('untrusted comment:')) throw Error(`Invalid signature file: ${filename}`);
    platforms[platform] = { signature, url: `https://github.com/${REPO}/releases/download/v${version}/${filename}` };
  }
  // Fallback for unrecognised Windows install types (e.g. portable builds): NSIS installs the managed app.
  platforms['windows-x86_64'] = platforms['windows-x86_64-nsis'];
  return { version, notes: `StarNexus Key Tool v${version}`, pub_date: pubDate, platforms };
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const [directory, output, tag] = process.argv.slice(2);
  const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
  const version = JSON.parse(fs.readFileSync(path.join(root, 'package.json'), 'utf8')).version;
  const tauri = JSON.parse(fs.readFileSync(path.join(root, 'src-tauri/tauri.conf.json'), 'utf8'));
  const cargo = fs.readFileSync(path.join(root, 'src-tauri/Cargo.toml'), 'utf8').match(/^version = "([^"]+)"/m)?.[1];
  if (!directory || !output || tag !== `v${version}` || tauri.version !== version || cargo !== version) throw Error('Tag and package/Tauri/Cargo versions must match');
  fs.writeFileSync(output, `${JSON.stringify(buildManifest(version, directory), null, 2)}\n`);
}
