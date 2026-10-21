// Node ESM resolve hook used by gen_fixtures.mjs.
//
// ez-tree's sources are written for a bundler (Vite) and import sibling
// modules without a file extension (`import RNG from './rng'`). Node's ESM
// loader requires explicit extensions, so retry extensionless relative
// specifiers with `.js` appended. The original sources stay byte-identical.

export async function resolve(specifier, context, nextResolve) {
  try {
    return await nextResolve(specifier, context);
  } catch (err) {
    const relative = specifier.startsWith('./') || specifier.startsWith('../');
    if (relative && !/\.[cm]?js$/.test(specifier)) {
      return nextResolve(`${specifier}.js`, context);
    }
    throw err;
  }
}
