import type { CityFile } from "../types";

// Deterministic radial tree: root at the origin, directories as branch
// forks, files as leaf slots. Every leaf gets an equal angular share in
// path-sorted order, so the same commit always grows the same tree.

export interface TreeDir {
  path: string;
  name: string;
  x: number;
  z: number;
  depth: number;
  fileCount: number;
  // how far the subtree's leaves reach from the fork, in layout units —
  // the scene projects this to screen pixels to decide when the directory
  // is prominent enough to deserve its label
  radius: number;
}

export interface TreeEdge {
  childPath?: string;
  childFileId?: number;
  points: { x: number; z: number }[];
}

export interface TreeLayout {
  radius: number;
  leaf: Map<number, { x: number; z: number }>;
  dirs: TreeDir[];
  edges: TreeEdge[];
}

interface Node {
  name: string;
  path: string;
  depth: number;
  children: Map<string, Node>;
  files: CityFile[];
  leafCount: number;
  angle: number;
}

export function computeTreeLayout(files: CityFile[]): TreeLayout {
  const root: Node = { name: "", path: "", depth: 0, children: new Map(), files: [], leafCount: 0, angle: 0 };
  const sorted = [...files].sort((a, b) => (a.path < b.path ? -1 : 1));
  for (const file of sorted) {
    const parts = file.path.split("/").filter(Boolean);
    let node = root;
    for (const part of parts.slice(0, -1)) {
      let next = node.children.get(part);
      if (!next) {
        next = {
          name: part,
          path: node.path ? `${node.path}/${part}` : part,
          depth: node.depth + 1,
          children: new Map(),
          files: [],
          leafCount: 0,
          angle: 0
        };
        node.children.set(part, next);
      }
      node = next;
    }
    node.files.push(file);
  }

  let maxDepth = 1;
  const countLeaves = (node: Node): number => {
    node.leafCount = node.files.length;
    if (node.files.length > 0) maxDepth = Math.max(maxDepth, node.depth + 1);
    for (const child of node.children.values()) {
      node.leafCount += countLeaves(child);
    }
    return node.leafCount;
  };
  countLeaves(root);

  const total = Math.max(root.leafCount, 1);
  const radius = Math.max(55, Math.sqrt(total) * 4);
  const step = radius / Math.max(maxDepth, 1);
  const slotAngle = (Math.PI * 2) / total;

  const layout: TreeLayout = { radius, leaf: new Map(), dirs: [], edges: [] };
  const polar = (r: number, angle: number) => ({ x: r * Math.cos(angle), z: r * Math.sin(angle) });

  const sampleEdge = (r1: number, a1: number, r2: number, a2: number): { x: number; z: number }[] => {
    // straight spokes out of the root, eased sweeps between rings
    const from = r1 < 1e-6 ? a2 : a1;
    let delta = a2 - from;
    while (delta > Math.PI) delta -= Math.PI * 2;
    while (delta < -Math.PI) delta += Math.PI * 2;
    const points: { x: number; z: number }[] = [];
    const SEGMENTS = 8;
    for (let s = 0; s <= SEGMENTS; s++) {
      const t = s / SEGMENTS;
      const ease = t * t * (3 - 2 * t);
      points.push(polar(r1 + (r2 - r1) * t, from + delta * ease));
    }
    return points;
  };

  let cursor = 0;
  const place = (node: Node) => {
    node.angle = (cursor + node.leafCount / 2) * slotAngle;
    const dirs = [...node.children.values()].sort((a, b) => (a.name < b.name ? -1 : 1));
    const files = [...node.files].sort((a, b) => (a.path < b.path ? -1 : 1));
    for (const child of dirs) {
      place(child);
    }
    for (const file of files) {
      const angle = (cursor + 0.5) * slotAngle;
      cursor += 1;
      const pos = polar((node.depth + 1) * step, angle);
      layout.leaf.set(file.id, pos);
      layout.edges.push({
        childFileId: file.id,
        points: sampleEdge(node.depth * step, node.angle, (node.depth + 1) * step, angle)
      });
    }
    if (node.path !== "") {
      const pos = polar(node.depth * step, node.angle);
      layout.dirs.push({
        path: node.path,
        name: node.name,
        x: pos.x,
        z: pos.z,
        depth: node.depth,
        fileCount: node.leafCount,
        radius: 0
      });
    }
  };
  place(root);

  // parent→dir edges need final angles, so wire them after placement
  const nodeByPath = new Map<string, Node>();
  const collect = (node: Node) => {
    nodeByPath.set(node.path, node);
    for (const child of node.children.values()) collect(child);
  };
  collect(root);
  for (const dir of layout.dirs) {
    const node = nodeByPath.get(dir.path)!;
    const parentPath = dir.path.includes("/") ? dir.path.slice(0, dir.path.lastIndexOf("/")) : "";
    const parent = nodeByPath.get(parentPath)!;
    layout.edges.push({
      childPath: dir.path,
      points: sampleEdge(parent.depth * step, parent.angle, node.depth * step, node.angle)
    });
  }

  // subtree reach: every leaf stretches the radius of each ancestor dir
  const dirByPath = new Map(layout.dirs.map((dir) => [dir.path, dir]));
  for (const file of files) {
    const pos = layout.leaf.get(file.id);
    if (!pos) continue;
    const parts = file.path.split("/").filter(Boolean);
    let path = "";
    for (let i = 0; i < parts.length - 1; i++) {
      path = path ? `${path}/${parts[i]}` : parts[i];
      const dir = dirByPath.get(path);
      if (!dir) continue;
      dir.radius = Math.max(dir.radius, Math.hypot(pos.x - dir.x, pos.z - dir.z));
    }
  }

  return layout;
}
