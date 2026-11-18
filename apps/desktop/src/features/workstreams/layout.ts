import { crossings, labelAt, route, type Pt, type Rect } from "./route";

/**
 * Graph layout: host lanes as horizontal bands, node placement by ELK's layered algorithm
 * (one partition per lane, top to bottom), stored manual positions override it, and edges are
 * routed orthogonally around every node (./route).
 */

export interface LayoutInput {
  lanes: { host: string }[];
  nodes: { id: string; host: string; w: number; h: number; pos?: Pt | null }[];
  edges: { id: string; from: string; to: string }[];
}
export interface Laid {
  width: number;
  height: number;
  lanes: { host: string; y: number; h: number }[];
  nodes: Record<string, Rect>;
  edges: Record<string, { points: Pt[]; label: Pt }>;
}

export const LANE_LABEL = 190;
const LANE_PAD = 26;
const LANE_GAP = 16;
const MIN_LANE = 130;

type ElkNode = { id: string; x?: number; y?: number; width: number; height: number; layoutOptions?: Record<string, string> };

/** ELK placement (code-split). Returns node top-left corners in ELK's own space. */
export async function elkPlace(input: LayoutInput): Promise<Record<string, Pt>> {
  const { default: ELK } = await import("elkjs/lib/elk.bundled.js");
  const elk = new ELK();
  const laneIdx = new Map(input.lanes.map((l, i) => [l.host, i]));
  const g = await elk.layout({
    id: "root",
    layoutOptions: {
      "elk.algorithm": "layered",
      "elk.direction": "DOWN",
      "elk.edgeRouting": "ORTHOGONAL",
      "elk.partitioning.activate": "true",
      "elk.spacing.nodeNode": "70",
      "elk.layered.spacing.nodeNodeBetweenLayers": "90",
      "elk.layered.nodePlacement.strategy": "BRANDES_KOEPF",
    },
    children: input.nodes.map<ElkNode>((n) => ({ id: n.id, width: n.w, height: n.h, layoutOptions: { "elk.partitioning.partition": String(laneIdx.get(n.host) ?? 0) } })),
    edges: input.edges.filter((e) => e.from !== e.to).map((e) => ({ id: e.id, sources: [e.from], targets: [e.to] })),
  });
  const out: Record<string, Pt> = {};
  for (const c of (g.children ?? []) as ElkNode[]) out[c.id] = { x: c.x ?? 0, y: c.y ?? 0 };
  return out;
}

/**
 * Turn placements into the drawn layout: nodes are kept inside their host's lane band (lanes stack
 * top to bottom in `lanes` order), manual positions win, then every edge is routed.
 */
export function arrange(input: LayoutInput, placed: Record<string, Pt>): Laid {
  const lanes: Laid["lanes"] = [];
  const nodes: Record<string, Rect> = {};
  let y = 12;
  for (const l of input.lanes) {
    const mine = input.nodes.filter((n) => n.host === l.host);
    const auto = mine.filter((n) => !n.pos);
    const minY = Math.min(...auto.map((n) => placed[n.id]?.y ?? 0));
    let bottom = y + MIN_LANE;
    for (const n of mine) {
      const p = n.pos ? { x: n.pos.x, y: y + n.pos.y } : { x: LANE_LABEL + (placed[n.id]?.x ?? 0), y: y + LANE_PAD + ((placed[n.id]?.y ?? 0) - (Number.isFinite(minY) ? minY : 0)) };
      nodes[n.id] = { x: Math.max(LANE_LABEL, p.x), y: Math.max(y + 8, p.y), w: n.w, h: n.h };
      bottom = Math.max(bottom, nodes[n.id].y + n.h + LANE_PAD);
    }
    lanes.push({ host: l.host, y, h: bottom - y });
    y = bottom + LANE_GAP;
  }
  const rects = Object.values(nodes);
  const edges: Laid["edges"] = {};
  // Spread the ports of a node's edges along its side so parallel edges do not share a line.
  const outs = new Map<string, string[]>(), ins = new Map<string, string[]>();
  for (const e of input.edges) {
    outs.set(e.from, [...(outs.get(e.from) ?? []), e.id]);
    ins.set(e.to, [...(ins.get(e.to) ?? []), e.id]);
  }
  const frac = (list: string[] | undefined, id: string) => (list && list.length > 1 ? 0.25 + (0.5 * list.indexOf(id)) / (list.length - 1) : 0.5);
  const labels: Pt[] = [];
  for (const e of input.edges) {
    const a = nodes[e.from], b = nodes[e.to];
    if (!a || !b) continue;
    const points = route(a, b, rects, frac(outs.get(e.from), e.id), frac(ins.get(e.to), e.id));
    const label = labelAt(points, labels);
    labels.push(label);
    edges[e.id] = { points, label };
  }
  const width = Math.max(980, ...rects.map((r) => r.x + r.w + 60));
  return { width, height: Math.max(y, 300), lanes, nodes, edges };
}

export async function layoutGraph(input: LayoutInput): Promise<Laid> {
  const needAuto = input.nodes.some((n) => !n.pos);
  return arrange(input, needAuto ? await elkPlace(input) : {});
}

/** Total node crossings of all routed edges (0 by construction; tested). */
export const totalCrossings = (l: Laid): number => Object.values(l.edges).reduce((c, e) => c + crossings(e.points, Object.values(l.nodes)), 0);
