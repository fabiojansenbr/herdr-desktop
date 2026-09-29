#!/usr/bin/env node
// Deterministic placeholder icon for the 001 proof (packaging/branding belongs to 008).
// Writes src-tauri/icons/icon.png: 128x128 RGBA, dark square with an amber "H" bar motif.
import { deflateSync } from "node:zlib";
import { writeFileSync, mkdirSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const size = 128;
const out = resolve(dirname(fileURLToPath(import.meta.url)), "..", "src-tauri", "icons", "icon.png");
mkdirSync(dirname(out), { recursive: true });

const raw = Buffer.alloc((size * 4 + 1) * size);
for (let y = 0; y < size; y++) {
  raw[y * (size * 4 + 1)] = 0; // filter: none
  for (let x = 0; x < size; x++) {
    const o = y * (size * 4 + 1) + 1 + x * 4;
    const inLeft = x >= 24 && x < 40 && y >= 24 && y < 104;
    const inRight = x >= 88 && x < 104 && y >= 24 && y < 104;
    const inBar = y >= 56 && y < 72 && x >= 40 && x < 88;
    const on = inLeft || inRight || inBar;
    raw[o] = on ? 0xe5 : 0x10;
    raw[o + 1] = on ? 0xc0 : 0x14;
    raw[o + 2] = on ? 0x7b : 0x18;
    raw[o + 3] = 0xff;
  }
}

const crcTable = new Int32Array(256).map((_, n) => {
  let c = n;
  for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
  return c;
});
const crc32 = (buf) => {
  let c = -1;
  for (const b of buf) c = crcTable[(c ^ b) & 0xff] ^ (c >>> 8);
  return (c ^ -1) >>> 0;
};
const chunk = (type, data) => {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length);
  const typed = Buffer.concat([Buffer.from(type, "ascii"), data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(typed));
  return Buffer.concat([len, typed, crc]);
};
const ihdr = Buffer.alloc(13);
ihdr.writeUInt32BE(size, 0);
ihdr.writeUInt32BE(size, 4);
ihdr[8] = 8; // bit depth
ihdr[9] = 6; // RGBA
const png = Buffer.concat([
  Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
  chunk("IHDR", ihdr),
  chunk("IDAT", deflateSync(raw, { level: 9 })),
  chunk("IEND", Buffer.alloc(0)),
]);
writeFileSync(out, png);
console.log(`wrote ${out} (${png.length} bytes)`);
