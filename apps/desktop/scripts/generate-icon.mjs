import { mkdirSync, writeFileSync } from 'node:fs';
const size = 32;
const header = Buffer.alloc(22);
header.writeUInt16LE(1, 2); header.writeUInt16LE(1, 4);
header[6] = size; header[7] = size; header.writeUInt16LE(1, 10); header.writeUInt16LE(32, 12);
const bitmap = Buffer.alloc(40 + size * size * 4 + size * 4);
bitmap.writeUInt32LE(40, 0); bitmap.writeInt32LE(size, 4); bitmap.writeInt32LE(size * 2, 8);
bitmap.writeUInt16LE(1, 12); bitmap.writeUInt16LE(32, 14); bitmap.writeUInt32LE(size * size * 4, 20);
for (let y = 0; y < size; y++) for (let x = 0; x < size; x++) {
  const bar = (x >= 8 && x < 11 && y >= 15 && y < 25) || (x >= 14 && x < 17 && y >= 8 && y < 25) || (x >= 20 && x < 23 && y >= 18 && y < 25);
  const rgb = bar ? [53, 100, 71] : [209, 237, 206];
  const offset = 40 + ((size - 1 - y) * size + x) * 4;
  bitmap[offset] = rgb[2]; bitmap[offset + 1] = rgb[1]; bitmap[offset + 2] = rgb[0]; bitmap[offset + 3] = 255;
}
header.writeUInt32LE(bitmap.length, 14); header.writeUInt32LE(header.length, 18);
const directory = new URL('../src-tauri/icons/', import.meta.url);
mkdirSync(directory, { recursive: true });
writeFileSync(new URL('icon.ico', directory), Buffer.concat([header, bitmap]));
