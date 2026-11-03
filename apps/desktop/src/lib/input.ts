// Mouse/keyboard encoding expected by the core (`mask = type | button << 3`).

export const MOUSE = { MOVE: 0, DOWN: 1, UP: 2, WHEEL: 3, TRACKPAD: 4 } as const;
const BUTTON = { LEFT: 1, RIGHT: 2, WHEEL: 4, BACK: 8, FORWARD: 16 } as const;

/** DOM `MouseEvent.button` (0 left, 1 middle, 2 right, 3 back, 4 forward) to core button bits. */
export function buttonBits(domButton: number): number {
  return [BUTTON.LEFT, BUTTON.WHEEL, BUTTON.RIGHT, BUTTON.BACK, BUTTON.FORWARD][domButton] ?? BUTTON.LEFT;
}

export function mouseMask(type: number, domButton: number): number {
  return type | (buttonBits(domButton) << 3);
}

const LINE_PX = 40;
const STEP_PX = 100;

/**
 * Convert a wheel event to whole notches. Pixel-precise devices (trackpads) accumulate until a
 * full notch is reached so slow scrolls are not lost. Positive y = scroll up, as the core expects.
 */
export function wheelSteps(e: { deltaX: number; deltaY: number; deltaMode: number }, acc: { x: number; y: number }) {
  const scale = e.deltaMode === 1 ? LINE_PX : e.deltaMode === 2 ? STEP_PX * 3 : 1;
  let ax = acc.x + e.deltaX * scale;
  let ay = acc.y + e.deltaY * scale;
  const take = (v: number) => (Math.abs(v) >= STEP_PX ? Math.trunc(v / STEP_PX) : 0);
  const sx = take(ax);
  const sy = take(ay);
  ax -= sx * STEP_PX;
  ay -= sy * STEP_PX;
  return { x: -sx, y: -sy, acc: { x: ax, y: ay } };
}

/** Lock key state bits: 1 << 1 caps lock, 1 << 2 num lock. */
export function lockModes(e: { getModifierState(k: string): boolean }): number {
  let m = 0;
  if (e.getModifierState("CapsLock")) m |= 1 << 1;
  if (e.getModifierState("NumLock")) m |= 1 << 2;
  return m;
}
