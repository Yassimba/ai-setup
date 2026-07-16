import * as THREE from "three";
import type { Touch } from "../types";

// Shared scene vocabulary. The touch colors are the meaning the HUD legend
// promises; both scenes must draw them identically. Ambient colors (ground,
// unvisited, ghost) stay per-scene tuning.
export const SKY = new THREE.Color("#12151c");
export const EMBER = new THREE.Color("#ff9e5e");

export const touchColors: Record<Touch | "selected", THREE.Color> = {
  hit: new THREE.Color("#8fb45f"),
  // chromatic enough to read as blue on lit terrain columns — a paler tint
  // washed out to white and stopped matching the HUD legend
  read: new THREE.Color("#a5c8f1"),
  edit: new THREE.Color("#f0ad5a"),
  selected: new THREE.Color("#f6ead2")
};

// Distance along `dir` that fits every point inside the camera frustum.
// Returns null while the viewport has no usable aspect (hidden pane, tab in
// the background, mid-layout) — fitting then would park the camera at NaN
// forever; callers must retry once a real resize lands.
export function fitDistance(
  camera: THREE.PerspectiveCamera,
  dir: THREE.Vector3,
  points: Iterable<THREE.Vector3>
): number | null {
  if (!Number.isFinite(camera.aspect) || camera.aspect <= 0) return null;
  const forward = dir.clone().negate();
  const right = new THREE.Vector3().crossVectors(forward, new THREE.Vector3(0, 1, 0)).normalize();
  const up = new THREE.Vector3().crossVectors(right, forward);
  const tanV = Math.tan(THREE.MathUtils.degToRad(camera.fov) / 2);
  const tanH = tanV * camera.aspect;
  let distance = 0;
  for (const point of points) {
    const depth = point.dot(forward);
    distance = Math.max(
      distance,
      Math.abs(point.dot(right)) / tanH - depth,
      Math.abs(point.dot(up)) / tanV - depth
    );
  }
  return distance;
}

// Pan the camera (position + orbit target together, so the view direction
// holds) until `world` projects inside the viewport's safe area. The right
// margin reserves room for the inspector panel, which would otherwise sit
// exactly on top of the leaf the user just selected.
export function ensureVisible(
  camera: THREE.PerspectiveCamera,
  controls: { target: THREE.Vector3; update: () => void },
  world: THREE.Vector3,
  viewW: number,
  viewH: number,
  reservedRight: number
) {
  if (viewW === 0 || viewH === 0) return;
  const forward = camera.getWorldDirection(new THREE.Vector3());
  const depth = world.clone().sub(camera.position).dot(forward);
  if (depth <= 0) return; // behind the camera: panning math breaks down
  const projected = world.clone().project(camera);
  const sx = ((projected.x + 1) / 2) * viewW;
  const sy = ((1 - projected.y) / 2) * viewH;
  const safeL = 48;
  const safeR = Math.max(safeL + 60, viewW - reservedRight - 48);
  const safeT = 120;
  const safeB = viewH - 100;
  const targetX = Math.min(Math.max(sx, safeL), safeR);
  const targetY = Math.min(Math.max(sy, safeT), safeB);
  if (targetX === sx && targetY === sy) return;
  const tanV = Math.tan(THREE.MathUtils.degToRad(camera.fov) / 2);
  const tanH = tanV * camera.aspect;
  const right = new THREE.Vector3().crossVectors(forward, new THREE.Vector3(0, 1, 0)).normalize();
  const up = new THREE.Vector3().crossVectors(right, forward);
  // moving the camera right shifts the point left on screen, and vice versa
  const pan = right
    .multiplyScalar(((sx - targetX) * 2 * depth * tanH) / viewW)
    .addScaledVector(up, ((targetY - sy) * 2 * depth * tanV) / viewH);
  camera.position.add(pan);
  controls.target.add(pan);
  controls.update();
}

// One-line hover readout, driven imperatively from the scenes' pointermove
// handlers so hovering never re-renders the React tree.
export class SceneTip {
  private readonly el: HTMLDivElement;
  private readonly pathEl: HTMLSpanElement;
  private readonly metaEl: HTMLSpanElement;

  constructor(private readonly host: HTMLElement) {
    this.el = document.createElement("div");
    this.el.className = "scene-tip";
    this.pathEl = document.createElement("span");
    this.metaEl = document.createElement("span");
    this.metaEl.className = "dim";
    this.el.append(this.pathEl, this.metaEl);
    host.appendChild(this.el);
  }

  show(path: string, meta: string, clientX: number, clientY: number) {
    this.pathEl.textContent = path;
    this.metaEl.textContent = ` · ${meta}`;
    this.el.style.display = "block";
    const bounds = this.host.getBoundingClientRect();
    const x = clientX - bounds.left;
    const y = clientY - bounds.top;
    const left = Math.min(x + 14, Math.max(0, bounds.width - this.el.offsetWidth - 8));
    const top = Math.min(y + 16, Math.max(0, bounds.height - this.el.offsetHeight - 8));
    this.el.style.left = `${left}px`;
    this.el.style.top = `${top}px`;
  }

  hide() {
    this.el.style.display = "none";
  }

  dispose() {
    this.el.remove();
  }
}

export const prefersReducedMotion = () =>
  typeof window !== "undefined" && window.matchMedia("(prefers-reduced-motion: reduce)").matches;

export function disposeGroup(group: THREE.Group) {
  group.traverse((obj) => {
    if (obj instanceof THREE.Mesh || obj instanceof THREE.Line || obj instanceof THREE.Sprite) {
      obj.geometry?.dispose();
      const mat = obj.material as THREE.Material | THREE.Material[];
      if (Array.isArray(mat)) mat.forEach(disposeMaterial);
      else if (mat) disposeMaterial(mat);
    }
  });
}

function disposeMaterial(mat: THREE.Material) {
  // Material.dispose() does not free assigned textures; module-cached
  // textures (fireflyTexture/haloTexture) are marked shared and must survive.
  const map = (mat as THREE.Material & { map?: THREE.Texture | null }).map;
  if (map && !map.userData.shared) map.dispose();
  mat.dispose();
}
