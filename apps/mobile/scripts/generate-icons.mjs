// Draws the placeholder app icon and splash marks from the design tokens, with no image
// dependencies. Run with `node scripts/generate-icons.mjs` (Node 24 or later, which can import
// the TypeScript token file directly). The output in `assets/` is committed.
//
// The mark is a lens: a ring with a solid centre. Replace these files when real artwork exists.

import { mkdirSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { deflateSync } from 'node:zlib';

import { tokens } from '../src/theme/tokens.ts';

const assets = join(dirname(fileURLToPath(import.meta.url)), '..', 'assets');

const SAMPLES = 4;

function parseColor(hex) {
  const value = hex.replace('#', '');
  return [0, 2, 4].map((offset) => parseInt(value.slice(offset, offset + 2), 16));
}

const crcTable = new Uint32Array(256).map((_, index) => {
  let value = index;
  for (let bit = 0; bit < 8; bit += 1) {
    value = value & 1 ? 0xedb88320 ^ (value >>> 1) : value >>> 1;
  }
  return value >>> 0;
});

function crc32(buffer) {
  let crc = 0xffffffff;
  for (const byte of buffer) {
    crc = crcTable[(crc ^ byte) & 0xff] ^ (crc >>> 8);
  }
  return (crc ^ 0xffffffff) >>> 0;
}

function chunk(type, data) {
  const body = Buffer.concat([Buffer.from(type, 'ascii'), data]);
  const length = Buffer.alloc(4);
  length.writeUInt32BE(data.length);
  const checksum = Buffer.alloc(4);
  checksum.writeUInt32BE(crc32(body));
  return Buffer.concat([length, body, checksum]);
}

/** Encodes RGBA pixels as a PNG, with or without the alpha channel. */
function encodePng(size, pixels, withAlpha) {
  const channels = withAlpha ? 4 : 3;
  const raw = Buffer.alloc(size * (1 + size * channels));
  for (let y = 0; y < size; y += 1) {
    const row = y * (1 + size * channels);
    raw[row] = 0;
    for (let x = 0; x < size; x += 1) {
      const source = (y * size + x) * 4;
      const target = row + 1 + x * channels;
      raw[target] = pixels[source];
      raw[target + 1] = pixels[source + 1];
      raw[target + 2] = pixels[source + 2];
      if (withAlpha) {
        raw[target + 3] = pixels[source + 3];
      }
    }
  }

  const header = Buffer.alloc(13);
  header.writeUInt32BE(size, 0);
  header.writeUInt32BE(size, 4);
  header[8] = 8;
  header[9] = withAlpha ? 6 : 2;

  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk('IHDR', header),
    chunk('IDAT', deflateSync(raw, { level: 9 })),
    chunk('IEND', Buffer.alloc(0)),
  ]);
}

/** How much of the pixel at (x, y) the lens mark covers, from 0 to 1. */
function markCoverage(x, y, size, scale) {
  const centre = size / 2;
  const outer = size * 0.5 * scale;
  const inner = outer * 0.74;
  const core = outer * 0.4;

  let covered = 0;
  for (let sy = 0; sy < SAMPLES; sy += 1) {
    for (let sx = 0; sx < SAMPLES; sx += 1) {
      const dx = x + (sx + 0.5) / SAMPLES - centre;
      const dy = y + (sy + 0.5) / SAMPLES - centre;
      const distance = Math.hypot(dx, dy);
      if ((distance <= outer && distance >= inner) || distance <= core) {
        covered += 1;
      }
    }
  }
  return covered / (SAMPLES * SAMPLES);
}

function draw({ size, scale, foreground, background }) {
  const front = parseColor(foreground);
  const back = background ? parseColor(background) : null;
  const pixels = Buffer.alloc(size * size * 4);

  for (let y = 0; y < size; y += 1) {
    for (let x = 0; x < size; x += 1) {
      const coverage = markCoverage(x, y, size, scale);
      const offset = (y * size + x) * 4;
      for (let channel = 0; channel < 3; channel += 1) {
        pixels[offset + channel] = back
          ? Math.round(back[channel] + (front[channel] - back[channel]) * coverage)
          : front[channel];
      }
      pixels[offset + 3] = back ? 255 : Math.round(coverage * 255);
    }
  }
  return encodePng(size, pixels, !back);
}

mkdirSync(assets, { recursive: true });

// The App Store rejects icons with transparency, so the icon is opaque.
writeFileSync(
  join(assets, 'icon.png'),
  draw({
    size: 1024,
    scale: 0.56,
    foreground: tokens.color.dark.text,
    background: tokens.color.dark.background,
  }),
);

// Splash marks sit on the theme background set in app.config.ts.
writeFileSync(
  join(assets, 'splash-icon.png'),
  draw({ size: 512, scale: 0.9, foreground: tokens.color.light.text }),
);
writeFileSync(
  join(assets, 'splash-icon-dark.png'),
  draw({ size: 512, scale: 0.9, foreground: tokens.color.dark.text }),
);

console.log(`Wrote icon.png, splash-icon.png and splash-icon-dark.png to ${assets}`);
