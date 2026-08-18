const PIN_LENGTH = 6;

export interface PinEntry {
  digits: string;
  formatted: string;
  complete: boolean;
}

export function normalizePinEntry(value: string): PinEntry {
  const digits = value.replace(/\D/g, "").slice(0, PIN_LENGTH);
  return {
    digits,
    formatted: digits.length > 3 ? `${digits.slice(0, 3)} ${digits.slice(3)}` : digits,
    complete: digits.length === PIN_LENGTH,
  };
}
