// The look the reference's HUD sets (web/css/style.css): quiet graphite glass in the corners of the screen, taxi
// yellow for whatever is active, and the green of a Manhattan street sign.
export const INK = "#f6f5f2";
export const DIM = "#f6f5f2ad";
export const FAINT = "#f6f5f270";
export const GLASS = "#121316d9";
export const PLATE = "#10111470";
export const PANEL = "#101114f2";
/** A list's heading: the panel's colour, opaque, so that a list sliding under it does not show through. */
export const HEADING = "#111215";
export const HAIRLINE = "#ffffff1a";
export const WASH = "#ffffff17";
export const TAXI = "#ffc21a";
export const TAXI_INK = "#17140a";
export const SIGN = "#0b6a46";
export const NIGHT = "#0a1118";

/** "#rrggbb" with an alpha in 0…1. */
export function tint(color: string, alpha: number): string {
  return color.slice(0, 7) + Math.round(Math.max(0, Math.min(1, alpha)) * 255).toString(16).padStart(2, "0");
}
