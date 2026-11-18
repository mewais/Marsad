/**
 * Orthogonal edge routing around node rectangles: a sparse visibility grid built from node
 * borders (plus margins), searched with Dijkstra and a bend penalty. Paths never enter a node.
 */

export interface Rect {
  x: number;
  y: number;
  w: number;
  h: number;
}
export interface Pt {
  x: number;
  y: number;
}

const M = 14;
const inflate = (r: Rect, m: number): Rect => ({ x: r.x - m, y: r.y - m, w: r.w + 2 * m, h: r.h + 2 * m });
const inside = (p: Pt, r: Rect) => p.x > r.x && p.x < r.x + r.w && p.y > r.y && p.y < r.y + r.h;

/** Does the axis-aligned segment a→b pass through the interior of r? */
export function segmentHitsRect(a: Pt, b: Pt, r: Rect): boolean {
  const x1 = Math.min(a.x, b.x), x2 = Math.max(a.x, b.x), y1 = Math.min(a.y, b.y), y2 = Math.max(a.y, b.y);
  if (x1 === x2) return x1 > r.x && x1 < r.x + r.w && y2 > r.y && y1 < r.y + r.h;
  if (y1 === y2) return y1 > r.y && y1 < r.y + r.h && x2 > r.x && x1 < r.x + r.w;
  return true;
}

type Side = "top" | "bottom" | "left" | "right";
function port(r: Rect, side: Side, frac = 0.5): { at: Pt; out: Pt } {
  const cx = r.x + r.w * frac, cy = r.y + r.h / 2;
  switch (side) {
    case "top": return { at: { x: cx, y: r.y }, out: { x: cx, y: r.y - M } };
    case "bottom": return { at: { x: cx, y: r.y + r.h }, out: { x: cx, y: r.y + r.h + M } };
    case "left": return { at: { x: r.x, y: cy }, out: { x: r.x - M, y: cy } };
    case "right": return { at: { x: r.x + r.w, y: cy }, out: { x: r.x + r.w + M, y: cy } };
  }
}

function sides(a: Rect, b: Rect): [Side, Side] {
  const dy = b.y - (a.y + a.h), uy = a.y - (b.y + b.h);
  if (dy >= 2 * M) return ["bottom", "top"];
  if (uy >= 2 * M) return ["top", "bottom"];
  return b.x >= a.x ? ["right", "left"] : ["left", "right"];
}

export function route(src: Rect, dst: Rect, obstacles: Rect[], frac = 0.5, dstFrac = 0.5): Pt[] {
  const [s1, s2] = sides(src, dst);
  const a = port(src, s1, frac), b = port(dst, s2, dstFrac);
  const blocks = obstacles.map((r) => inflate(r, M - 2));
  const xs = new Set<number>([a.out.x, b.out.x]);
  const ys = new Set<number>([a.out.y, b.out.y]);
  for (const r of obstacles) {
    xs.add(r.x - M); xs.add(r.x + r.w + M); xs.add(r.x + r.w / 2);
    ys.add(r.y - M); ys.add(r.y + r.h + M); ys.add(r.y + r.h / 2);
  }
  const X = [...xs].sort((p, q) => p - q), Y = [...ys].sort((p, q) => p - q);
  const key = (i: number, j: number) => i * Y.length + j;
  const free = (i: number, j: number) => !blocks.some((r) => inside({ x: X[i], y: Y[j] }, r));
  const si = X.indexOf(a.out.x), sj = Y.indexOf(a.out.y), ti = X.indexOf(b.out.x), tj = Y.indexOf(b.out.y);
  // Dijkstra over (point, direction); 0 = horizontal, 1 = vertical.
  const dist = new Map<number, number>();
  const prev = new Map<number, number>();
  const sk = (k: number, d: number) => k * 2 + d;
  const queue: [number, number][] = [];
  const push = (s: number, c: number) => {
    queue.push([c, s]);
  };
  for (const d of [0, 1]) { dist.set(sk(key(si, sj), d), 0); push(sk(key(si, sj), d), 0); }
  let end = -1;
  while (queue.length) {
    queue.sort((p, q) => p[0] - q[0]);
    const [c, s] = queue.shift()!;
    if (c > (dist.get(s) ?? Infinity)) continue;
    const k = s >> 1, d = s & 1, i = Math.floor(k / Y.length), j = k % Y.length;
    if (i === ti && j === tj) { end = s; break; }
    for (const [di, dj, nd] of [[1, 0, 0], [-1, 0, 0], [0, 1, 1], [0, -1, 1]] as const) {
      const ni = i + di, nj = j + dj;
      if (ni < 0 || nj < 0 || ni >= X.length || nj >= Y.length || !free(ni, nj)) continue;
      const p = { x: X[i], y: Y[j] }, q = { x: X[ni], y: Y[nj] };
      if (blocks.some((r) => segmentHitsRect(p, q, r))) continue;
      const ns = sk(key(ni, nj), nd);
      const nc = c + Math.abs(q.x - p.x) + Math.abs(q.y - p.y) + (nd !== d ? 40 : 0);
      if (nc < (dist.get(ns) ?? Infinity)) { dist.set(ns, nc); prev.set(ns, s); push(ns, nc); }
    }
  }
  const pts: Pt[] = [b.at];
  if (end >= 0) {
    for (let s: number | undefined = end; s !== undefined; s = prev.get(s)) {
      const k = s >> 1;
      pts.push({ x: X[Math.floor(k / Y.length)], y: Y[k % Y.length] });
    }
  } else pts.push(b.out, { x: a.out.x, y: b.out.y }, a.out);
  pts.push(a.at);
  return simplify(pts.reverse());
}

function simplify(p: Pt[]): Pt[] {
  const out: Pt[] = [];
  for (const q of p) {
    const n = out.length;
    if (n && out[n - 1].x === q.x && out[n - 1].y === q.y) continue;
    if (n >= 2 && ((out[n - 2].x === out[n - 1].x && out[n - 1].x === q.x) || (out[n - 2].y === out[n - 1].y && out[n - 1].y === q.y))) out[n - 1] = q;
    else out.push(q);
  }
  return out;
}

export const pathD = (p: Pt[]) => p.map((q, i) => `${i ? "L" : "M"}${Math.round(q.x)} ${Math.round(q.y)}`).join(" ");

/** Where to put an edge's label: the middle of its longest horizontal run (they lie in the gaps between rows), else its longest segment. */
export function labelAt(p: Pt[], taken: Pt[] = []): Pt {
  let best = 0, at = { x: p[0].x, y: p[0].y };
  const clash = (q: Pt) => taken.some((t) => Math.abs(t.x - q.x) < 140 && Math.abs(t.y - q.y) < 24);
  for (const horizontalOnly of [true, false]) {
    for (let i = 1; i < p.length; i++) {
      if (horizontalOnly && p[i].y !== p[i - 1].y) continue;
      if (clash({ x: (p[i].x + p[i - 1].x) / 2, y: (p[i].y + p[i - 1].y) / 2 })) continue;
      const l = Math.abs(p[i].x - p[i - 1].x) + Math.abs(p[i].y - p[i - 1].y);
      if (l > best) { best = l; at = { x: (p[i].x + p[i - 1].x) / 2, y: (p[i].y + p[i - 1].y) / 2 }; }
    }
    if (best >= 90) break;
  }
  return at;
}

/** Every segment of `path` that enters a node it does not start or end at (should be none). */
export function crossings(path: Pt[], nodes: Rect[]): number {
  let n = 0;
  for (let i = 1; i < path.length; i++) for (const r of nodes) if (segmentHitsRect(path[i - 1], path[i], r)) n++;
  return n;
}
