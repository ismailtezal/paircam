// Writes src/generated/licences.json: the open-source packages the app ships with, read from the
// installed copies of the direct dependencies. Run with `npm run generate:licences` after adding,
// removing or upgrading a dependency.
//
// Code copied into the repository rather than installed is listed by hand below.

import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const manifest = JSON.parse(readFileSync(join(root, 'package.json'), 'utf8'));

/** Projects whose code was copied in and adapted, with their notices kept in the source files. */
const copied = [
  {
    name: 'React Native Reusables',
    version: 'components/ui',
    licence: 'MIT',
  },
];

const installed = Object.keys(manifest.dependencies ?? {}).map((name) => {
  const dependency = JSON.parse(
    readFileSync(join(root, 'node_modules', name, 'package.json'), 'utf8'),
  );
  const licence = typeof dependency.license === 'string' ? dependency.license : null;
  return { name, version: dependency.version, licence };
});

const entries = [...copied, ...installed].sort((left, right) =>
  left.name.localeCompare(right.name, 'en', { sensitivity: 'base' }),
);

const output = join(root, 'src', 'generated', 'licences.json');
mkdirSync(dirname(output), { recursive: true });
writeFileSync(output, `${JSON.stringify(entries, null, 2)}\n`);

console.log(`Wrote ${entries.length} entries to ${output}`);
