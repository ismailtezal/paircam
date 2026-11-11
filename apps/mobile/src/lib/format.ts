// Number and date formatting that follows the app's language. The numbers come out bare; the
// unit and its position belong to the translation catalog.

const BITS_PER_MEGABIT = 1_000_000;

/** Megabits per second with one decimal, such as "11.8" or "11,8". */
export function megabits(bitsPerSecond: number, language: string): string {
  return new Intl.NumberFormat(language, {
    minimumFractionDigits: 1,
    maximumFractionDigits: 1,
  }).format(bitsPerSecond / BITS_PER_MEGABIT);
}

/** A whole number, such as a frame rate. */
export function whole(value: number, language: string): string {
  return new Intl.NumberFormat(language, { maximumFractionDigits: 0 }).format(value);
}

/** A fraction from 0 to 1 as a whole percentage number, without the sign. */
export function percentNumber(fraction: number, language: string): string {
  return whole(Math.round(fraction * 100), language);
}

/** A calendar date, such as "11 Nov 2026". */
export function mediumDate(timestamp: number, language: string): string {
  return new Intl.DateTimeFormat(language, { dateStyle: 'medium' }).format(new Date(timestamp));
}
