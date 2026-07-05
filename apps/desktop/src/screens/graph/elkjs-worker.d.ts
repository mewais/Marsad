/**
 * Ambient module declaration for elkjs's in-process fake-Worker
 * implementation, imported directly (bypassing `elkjs`'s package root,
 * `lib/main.js`, which eagerly requires the OPTIONAL `web-worker` package in
 * a way bundlers hoist into a real import — see the long comment in
 * `layout.ts`'s `computeElkLayout`). elkjs ships no `.d.ts` for the minified
 * build, only for the unminified `elk-worker.js` — this covers the same
 * shape for the smaller `.min.js` bundle we actually import.
 */
declare module "elkjs/lib/elk-worker.min.js" {
  export class Worker {
    constructor(url: string);
  }
}
