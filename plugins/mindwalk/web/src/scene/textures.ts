import * as THREE from "three";

let fireflyMap: THREE.Texture | null = null;
export function fireflyTexture(): THREE.Texture {
  if (fireflyMap) return fireflyMap;
  const size = 64;
  const canvas = document.createElement("canvas");
  canvas.width = size;
  canvas.height = size;
  const ctx = canvas.getContext("2d")!;
  const g = ctx.createRadialGradient(size / 2, size / 2, 0, size / 2, size / 2, size / 2);
  g.addColorStop(0, "rgba(255,255,255,1)");
  g.addColorStop(0.25, "rgba(255,210,160,0.55)");
  g.addColorStop(1, "rgba(255,158,94,0)");
  ctx.fillStyle = g;
  ctx.fillRect(0, 0, size, size);
  fireflyMap = new THREE.CanvasTexture(canvas);
  fireflyMap.userData.shared = true; // module cache: disposeGroup must not free it
  return fireflyMap;
}

let haloMap: THREE.Texture | null = null;
export function haloTexture(): THREE.Texture {
  if (haloMap) return haloMap;
  const size = 128;
  const canvas = document.createElement("canvas");
  canvas.width = size;
  canvas.height = size;
  const ctx = canvas.getContext("2d")!;
  const g = ctx.createRadialGradient(size / 2, size / 2, 0, size / 2, size / 2, size / 2);
  g.addColorStop(0, "rgba(255,255,255,0.9)");
  g.addColorStop(0.4, "rgba(255,255,255,0.28)");
  g.addColorStop(1, "rgba(255,255,255,0)");
  ctx.fillStyle = g;
  ctx.fillRect(0, 0, size, size);
  haloMap = new THREE.CanvasTexture(canvas);
  haloMap.userData.shared = true; // module cache: disposeGroup must not free it
  return haloMap;
}

export function labelTexture(text: string): { texture: THREE.Texture; aspect: number } {
  const font = '500 30px "Schibsted Grotesk Variable", "PingFang SC", sans-serif';
  const measure = document.createElement("canvas").getContext("2d")!;
  measure.font = font;
  const width = Math.ceil(measure.measureText(text).width) + 24;
  const height = 44;
  const canvas = document.createElement("canvas");
  canvas.width = width * 2;
  canvas.height = height * 2;
  const ctx = canvas.getContext("2d")!;
  ctx.scale(2, 2);
  ctx.font = font;
  ctx.textBaseline = "middle";
  ctx.textAlign = "center";
  ctx.fillStyle = "rgba(197, 205, 222, 0.95)";
  ctx.fillText(text, width / 2, height / 2 + 1);
  const texture = new THREE.CanvasTexture(canvas);
  texture.anisotropy = 4;
  return { texture, aspect: width / height };
}
