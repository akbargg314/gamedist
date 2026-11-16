// Generates the app icons in src-tauri/icons/ without any image tooling:
//   deno run --allow-write=src-tauri/icons,public scripts/gen-icons.ts
// Produces PNGs (32, 128, 256, 512), icon.ico (PNG-embedded) and icon.icns
// (PNG-embedded), plus public/favicon.png. Replace with real artwork via
// `npm run tauri icon path/to/1024.png` whenever you like.

const OUT = new URL('../src-tauri/icons/', import.meta.url);
const PUBLIC = new URL('../public/', import.meta.url);

const CRC_TABLE = (() => {
  const t = new Uint32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    t[n] = c >>> 0;
  }
  return t;
})();

function crc32(bytes: Uint8Array): number {
  let c = 0xffffffff;
  for (const b of bytes) c = CRC_TABLE[(c ^ b) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}

function u32be(n: number): Uint8Array {
  return new Uint8Array([(n >>> 24) & 255, (n >>> 16) & 255, (n >>> 8) & 255, n & 255]);
}

function concat(parts: Uint8Array[]): Uint8Array {
  const out = new Uint8Array(parts.reduce((s, p) => s + p.length, 0));
  let o = 0;
  for (const p of parts) {
    out.set(p, o);
    o += p.length;
  }
  return out;
}

function chunk(type: string, data: Uint8Array): Uint8Array {
  const t = new TextEncoder().encode(type);
  return concat([u32be(data.length), t, data, u32be(crc32(concat([t, data])))]);
}

async function zlib(data: Uint8Array): Promise<Uint8Array> {
  const stream = new Blob([data as BlobPart]).stream().pipeThrough(new CompressionStream('deflate'));
  return new Uint8Array(await new Response(stream).arrayBuffer());
}

/** RGBA pixels for the GameDist mark: rounded square, blue gradient, white diamond. */
function draw(size: number): Uint8Array {
  const px = new Uint8Array(size * size * 4);
  const r = size * 0.22; // corner radius
  const ss = 4; // supersampling per axis
  for (let y = 0; y < size; y++) {
    for (let x = 0; x < size; x++) {
      let a = 0;
      let cr = 0;
      let cg = 0;
      let cb = 0;
      for (let sy = 0; sy < ss; sy++) {
        for (let sx = 0; sx < ss; sx++) {
          const fx = x + (sx + 0.5) / ss;
          const fy = y + (sy + 0.5) / ss;
          // Rounded-rect coverage.
          const dx = Math.max(r - fx, fx - (size - r), 0);
          const dy = Math.max(r - fy, fy - (size - r), 0);
          if (dx * dx + dy * dy > r * r) continue;
          // Background gradient (#2f7bf0 → #6a3df0).
          const t = (fx + fy) / (2 * size);
          let R = 47 + (106 - 47) * t;
          let G = 123 + (61 - 123) * t;
          let B = 240;
          // Diamond (◆) in the middle.
          const cx = Math.abs(fx - size / 2);
          const cy = Math.abs(fy - size / 2);
          const d = cx + cy;
          if (d < size * 0.3) {
            const inner = d < size * 0.13;
            R = inner ? 255 : 245;
            G = inner ? 181 : 248;
            B = inner ? 71 : 255;
          }
          a++;
          cr += R;
          cg += G;
          cb += B;
        }
      }
      const i = (y * size + x) * 4;
      if (a) {
        px[i] = Math.round(cr / a);
        px[i + 1] = Math.round(cg / a);
        px[i + 2] = Math.round(cb / a);
      }
      px[i + 3] = Math.round((255 * a) / (ss * ss));
    }
  }
  return px;
}

async function png(size: number): Promise<Uint8Array> {
  const rgba = draw(size);
  const raw = new Uint8Array(size * (size * 4 + 1));
  for (let y = 0; y < size; y++) {
    raw[y * (size * 4 + 1)] = 0; // filter: none
    raw.set(rgba.subarray(y * size * 4, (y + 1) * size * 4), y * (size * 4 + 1) + 1);
  }
  const ihdr = concat([u32be(size), u32be(size), new Uint8Array([8, 6, 0, 0, 0])]);
  return concat([
    new Uint8Array([137, 80, 78, 71, 13, 10, 26, 10]),
    chunk('IHDR', ihdr),
    chunk('IDAT', await zlib(raw)),
    chunk('IEND', new Uint8Array()),
  ]);
}

function ico(images: { size: number; data: Uint8Array }[]): Uint8Array {
  const header = new DataView(new ArrayBuffer(6));
  header.setUint16(2, 1, true);
  header.setUint16(4, images.length, true);
  const dir: Uint8Array[] = [];
  let offset = 6 + 16 * images.length;
  for (const img of images) {
    const e = new DataView(new ArrayBuffer(16));
    e.setUint8(0, img.size >= 256 ? 0 : img.size);
    e.setUint8(1, img.size >= 256 ? 0 : img.size);
    e.setUint16(4, 1, true); // planes
    e.setUint16(6, 32, true); // bpp
    e.setUint32(8, img.data.length, true);
    e.setUint32(12, offset, true);
    offset += img.data.length;
    dir.push(new Uint8Array(e.buffer));
  }
  return concat([new Uint8Array(header.buffer), ...dir, ...images.map((i) => i.data)]);
}

function icns(entries: { type: string; data: Uint8Array }[]): Uint8Array {
  const enc = new TextEncoder();
  const body = entries.map((e) => concat([enc.encode(e.type), u32be(e.data.length + 8), e.data]));
  const len = 8 + body.reduce((s, b) => s + b.length, 0);
  return concat([enc.encode('icns'), u32be(len), ...body]);
}

await Deno.mkdir(OUT, { recursive: true });
await Deno.mkdir(PUBLIC, { recursive: true });
const sizes = [16, 32, 48, 64, 128, 256, 512];
const pngs = new Map<number, Uint8Array>();
for (const s of sizes) pngs.set(s, await png(s));

await Deno.writeFile(new URL('32x32.png', OUT), pngs.get(32)!);
await Deno.writeFile(new URL('128x128.png', OUT), pngs.get(128)!);
await Deno.writeFile(new URL('128x128@2x.png', OUT), pngs.get(256)!);
await Deno.writeFile(new URL('icon.png', OUT), pngs.get(512)!);
await Deno.writeFile(
  new URL('icon.ico', OUT),
  ico([16, 32, 48, 64, 128, 256].map((size) => ({ size, data: pngs.get(size)! }))),
);
await Deno.writeFile(
  new URL('icon.icns', OUT),
  icns([
    { type: 'icp4', data: pngs.get(16)! },
    { type: 'icp5', data: pngs.get(32)! },
    { type: 'icp6', data: pngs.get(64)! },
    { type: 'ic07', data: pngs.get(128)! },
    { type: 'ic08', data: pngs.get(256)! },
    { type: 'ic09', data: pngs.get(512)! },
  ]),
);
await Deno.writeFile(new URL('favicon.png', PUBLIC), pngs.get(64)!);
console.log('icons written to', OUT.pathname);
