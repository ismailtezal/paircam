// Checks that every translation catalog has exactly the keys of the English source and uses the
// same placeholders in every string. TypeScript already checks the keys at build time; this also
// covers placeholders and runs without a compiler. Run with `npm run check:i18n`.

import { readdirSync, readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const SOURCE = 'en';
const locales = join(dirname(fileURLToPath(import.meta.url)), '..', 'src', 'i18n', 'locales');

/** Flattens a catalog to `{ 'a.b.c': 'text' }`. */
function flatten(node, prefix = '', result = {}) {
  for (const [key, value] of Object.entries(node)) {
    const path = prefix ? `${prefix}.${key}` : key;
    if (typeof value === 'string') {
      result[path] = value;
    } else {
      flatten(value, path, result);
    }
  }
  return result;
}

const placeholders = (text) =>
  [...text.matchAll(/{{\s*(\w+)\s*}}/g)].map((match) => match[1]).sort();

const catalogs = Object.fromEntries(
  readdirSync(locales)
    .filter((file) => file.endsWith('.json'))
    .map((file) => [
      file.replace(/\.json$/, ''),
      flatten(JSON.parse(readFileSync(join(locales, file), 'utf8'))),
    ]),
);

const source = catalogs[SOURCE];
if (!source) {
  console.error(`Missing the source catalog ${SOURCE}.json`);
  process.exit(1);
}

const problems = [];
for (const [language, catalog] of Object.entries(catalogs)) {
  if (language === SOURCE) {
    continue;
  }
  for (const key of Object.keys(source)) {
    if (!(key in catalog)) {
      problems.push(`${language}: missing "${key}"`);
    } else if (catalog[key].trim() === '') {
      problems.push(`${language}: empty "${key}"`);
    } else if (placeholders(catalog[key]).join() !== placeholders(source[key]).join()) {
      problems.push(`${language}: placeholders differ in "${key}"`);
    }
  }
  for (const key of Object.keys(catalog)) {
    if (!(key in source)) {
      problems.push(`${language}: extra "${key}"`);
    }
  }
}

if (problems.length > 0) {
  console.error(problems.join('\n'));
  process.exit(1);
}
console.log(
  `${Object.keys(catalogs).length} catalogs, ${Object.keys(source).length} keys each, all in step.`,
);
