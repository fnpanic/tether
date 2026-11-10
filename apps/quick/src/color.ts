/** WCAG relative luminance of `#rrggbb`, null if it is not such a colour. */
function luminance(hex: string): number | null {
  const m = /^#([0-9a-f]{2})([0-9a-f]{2})([0-9a-f]{2})$/i.exec(hex);
  if (!m) return null;
  const [r, g, b] = m.slice(1).map((h) => {
    const c = parseInt(h, 16) / 255;
    return c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

/** Text colour on a fill of `hex`: black or white, whichever has the higher WCAG contrast. */
export function onAccent(hex: string): "#000000" | "#ffffff" | null {
  const l = luminance(hex);
  if (l === null) return null;
  // Contrast with black against contrast with white.
  return (l + 0.05) / 0.05 > 1.05 / (l + 0.05) ? "#000000" : "#ffffff";
}
