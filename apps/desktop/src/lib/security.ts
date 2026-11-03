// Mirrors src-tauri/src/security.rs::check_password_strength so the UI can guide before submit.
// The Rust check is authoritative.

export function passwordStrength(pw: string): { ok: boolean; hint: string } {
  if (!pw) return { ok: false, hint: "At least 12 characters, mixing letters, digits and symbols." };
  const n = [...pw].length;
  if (n < 12) return { ok: false, hint: `${12 - n} more character${n === 11 ? "" : "s"} needed.` };
  if (pw.length > 128) return { ok: false, hint: "Use at most 128 characters." };
  const classes = [/[a-z]/, /[A-Z]/, /[0-9]/, /[^A-Za-z0-9]/].filter((r) => r.test(pw)).length;
  if (classes < 3) return { ok: false, hint: "Mix at least three of: lowercase, uppercase, digits, symbols." };
  return { ok: true, hint: "Strong password." };
}
