import { describe, expect, it } from "vitest";
import { calculateContentRect, normalizePoint } from "./geometry";

describe("video coordinate mapping", () => {
  it("accounts for fit letterboxing", () => {
    const rect = calculateContentRect(1366, 1024, 1920, 1080, "fit");
    expect(rect.left).toBeCloseTo(0);
    expect(rect.top).toBeCloseTo(127.84, 1);
    expect(normalizePoint(683, 512, rect, false)).toEqual({ u: 0.5, v: 0.5 });
    expect(normalizePoint(100, 20, rect, false)).toBeNull();
  });

  it("accounts for fill cropping", () => {
    const rect = calculateContentRect(1024, 768, 1920, 1080, "fill");
    expect(rect.left).toBeLessThan(0);
    const center = normalizePoint(512, 384, rect, true);
    expect(center?.u).toBeCloseTo(0.5);
    expect(center?.v).toBeCloseTo(0.5);
  });

  it("preserves source pixels in one-to-one mode", () => {
    expect(calculateContentRect(1000, 800, 400, 200, "one-to-one")).toEqual({
      left: 300,
      top: 300,
      width: 400,
      height: 200,
    });
  });
});
