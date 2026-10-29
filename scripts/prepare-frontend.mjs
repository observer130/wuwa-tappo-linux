import { cp, mkdir, readdir, readFile, rm, writeFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import path from 'node:path';
import {
  patchAbout,
  patchChestTimers,
  patchCookieStorage,
  patchDomesticSdkUrls,
  patchIndexLanguage,
  patchLoginReady,
} from './frontend-patches.mjs';

const root = fileURLToPath(new URL('../', import.meta.url));
const source = path.join(root, 'frontend');
const output = path.join(root, 'build/frontend');

const index = await readFile(path.join(source, 'index.html'), 'utf8');
const entry = index.match(/src="\.\/([^"]+\.js)"/)?.[1];
if (!entry) throw new Error('Original frontend entry module not found.');

const assets = await readdir(path.join(source, 'assets'));
const files = new Map(await Promise.all(
  assets.filter(name => name.endsWith('.js')).map(async name => [
    `assets/${name}`,
    await readFile(path.join(source, 'assets', name), 'utf8'),
  ]),
));
const original = files.get(entry);
if (!original) throw new Error(`Frontend entry module ${entry} is missing.`);
files.set(entry, patchCookieStorage(patchDomesticSdkUrls(original)));

function patchOne(description, matches, patch) {
  const selected = [...files].filter(([name, content]) => matches(name, content));
  if (selected.length !== 1) {
    throw new Error(`Expected one original ${description}, found ${selected.length}. Reconcile the upstream version before building.`);
  }
  const [name, content] = selected[0];
  files.set(name, patch(content));
}

patchOne('modal event bridge', name => /\/modalBridge-[^/]+\.js$/.test(name), patchLoginReady);
patchOne('About view', (_, content) => content.includes('about_content') && content.includes('系统配置'), patchAbout);
patchOne('chest timer view', (_, content) => content.includes('function Vn(){if(!A.isLoggedIn){Bt.value=0'), patchChestTimers);

await rm(output, { recursive: true, force: true });
await mkdir(output, { recursive: true });
await cp(source, output, { recursive: true });
await writeFile(path.join(output, 'index.html'), patchIndexLanguage(index));
for (const [name, content] of files) await writeFile(path.join(output, name), content);
console.log(`Prepared official CN frontend at ${output}`);
