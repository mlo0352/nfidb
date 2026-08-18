import { describe, expect, it } from "vitest";
import { normalizePinEntry } from "./pin-entry";

describe("PIN entry", () => {
  it("formats six digits and marks them ready for automatic submission", () => {
    expect(normalizePinEntry("123456")).toEqual({
      digits: "123456",
      formatted: "123 456",
      complete: true,
    });
  });

  it("accepts a pasted formatted PIN and ignores non-digits", () => {
    expect(normalizePinEntry("12a3- 45x6")).toEqual({
      digits: "123456",
      formatted: "123 456",
      complete: true,
    });
  });

  it("does not mark a partial PIN ready", () => {
    expect(normalizePinEntry("12345")).toEqual({
      digits: "12345",
      formatted: "123 45",
      complete: false,
    });
  });
});
