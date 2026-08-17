export type FitMode = "fit" | "fill" | "one-to-one";

export interface Rect {
  left: number;
  top: number;
  width: number;
  height: number;
}

export interface NormalizedPoint {
  u: number;
  v: number;
}

export function calculateContentRect(
  viewportWidth: number,
  viewportHeight: number,
  sourceWidth: number,
  sourceHeight: number,
  mode: FitMode,
): Rect {
  if (viewportWidth <= 0 || viewportHeight <= 0 || sourceWidth <= 0 || sourceHeight <= 0) {
    return { left: 0, top: 0, width: 0, height: 0 };
  }
  let scale: number;
  if (mode === "one-to-one") {
    scale = 1;
  } else {
    const horizontal = viewportWidth / sourceWidth;
    const vertical = viewportHeight / sourceHeight;
    scale = mode === "fit" ? Math.min(horizontal, vertical) : Math.max(horizontal, vertical);
  }
  const width = sourceWidth * scale;
  const height = sourceHeight * scale;
  return {
    left: (viewportWidth - width) / 2,
    top: (viewportHeight - height) / 2,
    width,
    height,
  };
}

export function normalizePoint(x: number, y: number, rect: Rect, clampToContent: boolean): NormalizedPoint | null {
  if (rect.width <= 0 || rect.height <= 0) {
    return null;
  }
  const u = (x - rect.left) / rect.width;
  const v = (y - rect.top) / rect.height;
  if (clampToContent) {
    return { u: clamp(u), v: clamp(v) };
  }
  if (u < 0 || u > 1 || v < 0 || v > 1) {
    return null;
  }
  return { u, v };
}

function clamp(value: number): number {
  return Math.max(0, Math.min(1, value));
}
