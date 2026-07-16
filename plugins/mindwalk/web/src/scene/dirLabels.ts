import * as THREE from "three";
import { labelTexture } from "./textures";

// Map-style LOD for directory names, shared by both scenes: a label shows
// while its subtree spans enough screen pixels — zoom out and only the big
// forks stay named, push in and deeper directories surface where you're
// looking. Two colliding labels drop whichever names fewer files.

export interface DirLabelEntry {
  name: string;
  x: number;
  z: number;
  /** world-space reach of the directory's subtree, drives the LOD cutoff */
  radius: number;
  fileCount: number;
  depth: number;
}

interface DirLabel {
  sprite: THREE.Sprite;
  entry: DirLabelEntry;
  aspect: number;
  target: number;
}

const LABEL_MIN_SUBTREE_PX = 60;
const LABEL_BUDGET = 120;

export class DirLabelSet {
  private readonly labels: DirLabel[] = [];
  private readonly y: number;
  private dirty = true;
  private readonly lastCamPos = new THREE.Vector3(Infinity, Infinity, Infinity);
  private readonly lastCamQuat = new THREE.Quaternion(0, 0, 0, 0);
  private lastViewW = 0;
  private lastViewH = 0;
  private readonly point = new THREE.Vector3();

  // sprites start invisible and fade in once the first projection pass runs;
  // alwaysOnTop labels (terrain) skip the depth test so mountains never bury
  // the district names
  constructor(entries: DirLabelEntry[], group: THREE.Group, y: number, alwaysOnTop = false) {
    this.y = y;
    const budget = [...entries].sort((a, b) => b.fileCount - a.fileCount).slice(0, LABEL_BUDGET);
    for (const entry of budget) {
      const { texture, aspect } = labelTexture(entry.name);
      const sprite = new THREE.Sprite(
        new THREE.SpriteMaterial({
          map: texture,
          transparent: true,
          opacity: 0,
          depthWrite: false,
          depthTest: !alwaysOnTop,
          toneMapped: false,
          fog: false
        })
      );
      if (alwaysOnTop) sprite.renderOrder = 20;
      sprite.visible = false;
      sprite.position.set(entry.x, y, entry.z);
      sprite.raycast = () => undefined;
      group.add(sprite);
      this.labels.push({ sprite, entry, aspect, target: 0 });
    }
  }

  markDirty() {
    this.dirty = true;
  }

  // reprojects subtrees whenever the camera or viewport moves, picks the
  // labels whose subtree is prominent on screen, drops collision losers
  updateTargets(camera: THREE.PerspectiveCamera, viewW: number, viewH: number) {
    if (this.labels.length === 0 || viewW === 0 || viewH === 0) return;
    const moved =
      this.dirty ||
      !camera.position.equals(this.lastCamPos) ||
      !camera.quaternion.equals(this.lastCamQuat) ||
      viewW !== this.lastViewW ||
      viewH !== this.lastViewH;
    if (!moved) return;
    this.dirty = false;
    this.lastCamPos.copy(camera.position);
    this.lastCamQuat.copy(camera.quaternion);
    this.lastViewW = viewW;
    this.lastViewH = viewH;

    const tanV = Math.tan(THREE.MathUtils.degToRad(camera.fov) / 2);
    const maxDim = Math.max(viewW, viewH);
    interface Candidate {
      label: DirLabel;
      sx: number;
      sy: number;
      pw: number;
      ph: number;
    }
    const candidates: Candidate[] = [];
    for (const label of this.labels) {
      this.point.set(label.entry.x, this.y, label.entry.z);
      const dist = this.point.distanceTo(camera.position);
      const pxPerWorld = viewH / (2 * dist * tanV);
      const subtreePx = label.entry.radius * pxPerWorld;
      this.point.project(camera);
      const sx = ((this.point.x + 1) / 2) * viewW;
      const sy = ((1 - this.point.y) / 2) * viewH;
      const onScreen = this.point.z < 1 && sx > -60 && sx < viewW + 60 && sy > -40 && sy < viewH + 40;
      // too small to matter, or so large we're inside it — either way the
      // name would float over unrelated geometry
      if (!onScreen || subtreePx < LABEL_MIN_SUBTREE_PX || subtreePx > maxDim * 1.6) {
        label.target = 0;
        continue;
      }
      // constant screen-size type, like map labels
      const ph = label.entry.depth <= 1 ? 15 : 13;
      const worldH = ph / pxPerWorld;
      label.sprite.scale.set(worldH * label.aspect, worldH, 1);
      candidates.push({ label, sx, sy, pw: ph * label.aspect, ph });
    }
    candidates.sort((a, b) => b.label.entry.fileCount - a.label.entry.fileCount);
    const kept: Candidate[] = [];
    for (const candidate of candidates) {
      const clash = kept.some(
        (other) =>
          Math.abs(other.sx - candidate.sx) < (other.pw + candidate.pw) / 2 + 14 &&
          Math.abs(other.sy - candidate.sy) < (other.ph + candidate.ph) / 2 + 10
      );
      candidate.label.target = clash ? 0 : 1;
      if (!clash) kept.push(candidate);
    }
  }

  // labels ease toward their LOD targets each frame
  ease(reduced: boolean) {
    for (const label of this.labels) {
      const material = label.sprite.material as THREE.SpriteMaterial;
      const diff = label.target - material.opacity;
      if (Math.abs(diff) > 0.02) {
        material.opacity = reduced ? label.target : material.opacity + diff * 0.16;
      } else if (material.opacity !== label.target) {
        material.opacity = label.target;
      }
      label.sprite.visible = material.opacity > 0.02;
    }
  }
}
