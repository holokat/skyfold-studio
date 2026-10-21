// Reference-data generator for the ez_tree Rust port.
//
// Runs the ORIGINAL ez-tree `src/lib/tree.js` (unmodified copy) under Node
// with a real three.js install and dumps compact per-preset / per-LOD
// summaries of the geometry it produces. The Rust integration tests in
// `crates/ez_tree/tests/` compare the port against these files.
//
// Usage (from the workspace root):
//
//   node crates/ez_tree/tools/gen_fixtures.mjs \
//     --ez-tree /path/to/ez-tree          # git clone of dgreenheck/ez-tree
//     --three   /path/to/node_modules/three
//     --work    /some/scratch/dir         # temp dir, recreated
//     [--out    crates/ez_tree/tests/fixtures]
//     [--full   /some/dir]                # also dump complete raw buffers
//
// Nothing is installed: the ez-tree lib sources are copied into --work,
// `presets/index.js` is replaced by a stub (Node cannot import the preset
// .json files without import attributes, and the generator itself never
// uses them), `node_modules/three` is symlinked to --three, and a resolve
// hook (resolve-hooks.mjs) adds the `.js` extensions the bundler-style
// imports omit.
//
// The three.js version matters for bit-exactness: three r180+ replaced
// Quaternion.slerp with a different (lerp-for-small-angles) algorithm. ez-tree
// pins three 0.167.1, whose slerp is the classic euclideanspace one that the
// Rust port reproduces, so use a pre-r180 install (0.179.1 was used).

import fs from 'node:fs';
import path from 'node:path';
import { register } from 'node:module';
import { pathToFileURL, fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));

function parseArgs(argv) {
  const args = {};
  for (let i = 0; i < argv.length; i += 2) {
    if (!argv[i].startsWith('--')) throw new Error(`unexpected argument ${argv[i]}`);
    args[argv[i].slice(2)] = argv[i + 1];
  }
  for (const required of ['ez-tree', 'three', 'work']) {
    if (!args[required]) throw new Error(`missing --${required}`);
  }
  args.out ??= path.join(here, '..', 'tests', 'fixtures');
  return args;
}

const args = parseArgs(process.argv.slice(2));
const repo = path.resolve(args['ez-tree']);
const threeDir = path.resolve(args.three);
const work = path.resolve(args.work);
const outDir = path.resolve(args.out);
const fullDir = args.full ? path.resolve(args.full) : null;

// --- Stage an unmodified copy of the generator --------------------------------
fs.rmSync(work, { recursive: true, force: true });
fs.mkdirSync(path.join(work, 'lib', 'presets'), { recursive: true });
fs.mkdirSync(path.join(work, 'node_modules'), { recursive: true });
for (const file of ['tree.js', 'rng.js', 'branch.js', 'enums.js', 'options.js', 'trellis.js']) {
  fs.copyFileSync(path.join(repo, 'src', 'lib', file), path.join(work, 'lib', file));
}
fs.writeFileSync(
  path.join(work, 'lib', 'presets', 'index.js'),
  `import TreeOptions from '../options';\n` +
    `export const TreePreset = {};\n` +
    `export function loadPreset() { return new TreeOptions(); }\n`,
);
fs.symlinkSync(threeDir, path.join(work, 'node_modules', 'three'), 'dir');

register(pathToFileURL(path.join(here, 'resolve-hooks.mjs')));

const { Tree } = await import(pathToFileURL(path.join(work, 'lib', 'tree.js')).href);
const { default: RNG } = await import(pathToFileURL(path.join(work, 'lib', 'rng.js')).href);
const threeVersion = JSON.parse(fs.readFileSync(path.join(threeDir, 'package.json'), 'utf8')).version;

// --- Helpers ------------------------------------------------------------------
const HEAD = 64; // vertices / indices kept from the start and the end of each buffer

// 9 significant digits round-trip any f32 exactly.
const f32 = (v) => Number(v.toPrecision(9));

function rows(array, itemSize, start, end) {
  const out = [];
  for (let i = start; i < end; i++) {
    const row = [];
    for (let c = 0; c < itemSize; c++) row.push(f32(array[i * itemSize + c]));
    out.push(row);
  }
  return out;
}

function summarize(geometry) {
  const pos = geometry.attributes.position.array;
  const nrm = geometry.attributes.normal.array;
  const uv = geometry.attributes.uv.array;
  const idx = geometry.index.array; // Uint16Array in ez-tree (wraps above 65535)
  const n = pos.length / 3;

  const min = [Infinity, Infinity, Infinity];
  const max = [-Infinity, -Infinity, -Infinity];
  const posSum = [0, 0, 0];
  const normalSum = [0, 0, 0];
  const uvSum = [0, 0];
  let posWeighted = 0;
  for (let i = 0; i < n; i++) {
    for (let c = 0; c < 3; c++) {
      const p = pos[i * 3 + c];
      min[c] = Math.min(min[c], p);
      max[c] = Math.max(max[c], p);
      posSum[c] += p;
      normalSum[c] += nrm[i * 3 + c];
    }
    uvSum[0] += uv[i * 2];
    uvSum[1] += uv[i * 2 + 1];
    // Order-sensitive checksum: catches permuted or shifted vertices that
    // plain sums would miss.
    posWeighted += ((i % 97) + 1) * (pos[i * 3] + 2 * pos[i * 3 + 1] + 3 * pos[i * 3 + 2]);
  }
  let indexSum = 0;
  let indexWeighted = 0;
  for (let i = 0; i < idx.length; i++) {
    indexSum += idx[i];
    indexWeighted += ((i % 89) + 1) * idx[i];
  }

  const headN = Math.min(HEAD, n);
  const tailStart = Math.max(0, n - HEAD);
  const headI = Math.min(HEAD, idx.length);
  const tailI = Math.max(0, idx.length - HEAD);
  return {
    vertexCount: n,
    indexCount: idx.length,
    bboxMin: n ? min : [0, 0, 0],
    bboxMax: n ? max : [0, 0, 0],
    posSum,
    normalSum,
    uvSum,
    posWeighted,
    indexSum,
    indexWeighted,
    head: {
      start: 0,
      positions: rows(pos, 3, 0, headN),
      normals: rows(nrm, 3, 0, headN),
      uvs: rows(uv, 2, 0, headN),
      indices: Array.from(idx.slice(0, headI)),
    },
    tail: {
      start: tailStart,
      positions: rows(pos, 3, tailStart, n),
      normals: rows(nrm, 3, tailStart, n),
      uvs: rows(uv, 2, tailStart, n),
      indexStart: tailI,
      indices: Array.from(idx.slice(tailI)),
    },
  };
}

function dumpFull(name, lod, part, geometry) {
  const base = path.join(fullDir, `${name}_lod${lod}_${part}`);
  const write = (suffix, typed) =>
    fs.writeFileSync(`${base}_${suffix}.bin`, Buffer.from(typed.buffer, typed.byteOffset, typed.byteLength));
  write('pos', geometry.attributes.position.array);
  write('nrm', geometry.attributes.normal.array);
  write('uv', geometry.attributes.uv.array);
  write('idx', geometry.index.array);
}

// --- Presets ------------------------------------------------------------------
fs.mkdirSync(outDir, { recursive: true });
if (fullDir) fs.mkdirSync(fullDir, { recursive: true });

const presetDir = path.join(repo, 'src', 'lib', 'presets');
const presetNames = fs
  .readdirSync(presetDir)
  .filter((f) => f.endsWith('.json') && f !== 'trellis.json')
  .map((f) => f.slice(0, -5))
  .sort();

for (const name of presetNames) {
  const json = JSON.parse(fs.readFileSync(path.join(presetDir, `${name}.json`), 'utf8'));
  const tree = new Tree(); // fresh options per preset: copy() merges into defaults
  tree.loadFromJson(json); // options.copy(json) + generate() (grows the skeleton)

  const lods = Tree.defaultLODLevels.map((level, lod) => {
    const geometry = tree.createGeometry(level.detail ?? {});
    if (fullDir) {
      dumpFull(name, lod, 'branches', geometry.branches);
      dumpFull(name, lod, 'leaves', geometry.leaves);
    }
    return {
      lod,
      detail: level.detail ?? {},
      branches: summarize(geometry.branches),
      leaves: summarize(geometry.leaves),
    };
  });

  const fixture = {
    preset: name,
    three: threeVersion,
    textureScaleY: tree.options.bark.textureScale.y,
    skeleton: { branches: tree.skeleton.branches.length, leaves: tree.skeleton.leaves.length },
    lods,
  };
  fs.writeFileSync(path.join(outDir, `${name}.json`), JSON.stringify(fixture) + '\n');
  const tris = lods.map((l) => (l.branches.indexCount + l.leaves.indexCount) / 3);
  console.log(`${name}: triangles per LOD ${tris.join(' / ')}`);
}

// --- RNG ------------------------------------------------------------------------
const rngSeeds = [0, 1, 42, 12345, 29919, -5, -987654321, 2147483647, 3000000000, -3000000000];
const rngFixture = rngSeeds.map((seed) => {
  const rng = new RNG(seed);
  const values = [];
  for (let i = 0; i < 16; i++) values.push(rng.random());
  // random(max, min) is (max - min) * r + min.
  const scaled = [];
  for (let i = 0; i < 4; i++) scaled.push(rng.random(0.3, -0.3));
  return { seed, values, scaled };
});
fs.writeFileSync(path.join(outDir, 'rng.json'), JSON.stringify(rngFixture, null, 1) + '\n');
console.log(`wrote ${presetNames.length} preset fixtures + rng.json to ${outDir} (three ${threeVersion})`);
