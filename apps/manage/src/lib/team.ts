// Team roster helpers (Team page): technician key parsing, name rules and the key fingerprint
// shown in both apps. Must match tether_trust::team (fingerprint, Roster::validate). team.test.ts.

export interface Member {
  name: string;
  /** Standard base64 of the technician's 32-byte Ed25519 public key. */
  key: string;
  write: boolean;
  audit: boolean;
}
export interface Roster {
  members: Member[];
}

export const MAX_MEMBERS = 500;

/** The 32 key bytes of a pasted public key (spaces and line breaks ignored), or null. */
export function parseKey(text: string): Uint8Array | null {
  const s = text.replace(/\s+/g, "");
  if (!/^[A-Za-z0-9+/]{43}=$/.test(s)) return null;
  const raw = Uint8Array.from(atob(s), (c) => c.charCodeAt(0));
  return raw.length === 32 ? raw : null;
}

/** `hex(sha256(key))`, first 16 hex digits, uppercase, groups of 4: `9F3A-12B0-C4D1-77E2`. */
export async function fingerprint(raw: Uint8Array): Promise<string> {
  const d = new Uint8Array(await crypto.subtle.digest("SHA-256", raw as BufferSource));
  const hex = [...d.slice(0, 8)].map((b) => b.toString(16).padStart(2, "0")).join("").toUpperCase();
  return hex.match(/.{4}/g)!.join("-");
}

/** Why a name is not accepted (server: 1–64 characters, no control characters), or null. */
export function nameProblem(name: string): string | null {
  const n = [...name].length;
  if (name.trim() !== name || n < 1 || n > 64) return "1–64 characters, no spaces at the start or end.";
  if (/\p{Cc}/u.test(name)) return "No control characters.";
  return null;
}

/** Why `key` cannot be added to `members`, or null. */
export function keyProblem(key: string, members: Member[]): string | null {
  const raw = parseKey(key);
  if (!raw) return "Not a technician key: paste the 44-character key the technician app shows.";
  const b64 = key.replace(/\s+/g, "");
  if (members.some((m) => m.key === b64)) return "This key is already on the team.";
  if (members.length >= MAX_MEMBERS) return `At most ${MAX_MEMBERS} technicians.`;
  return null;
}

/** Same members, flags and order. */
export const sameRoster = (a: Roster, b: Roster) => JSON.stringify(a.members) === JSON.stringify(b.members);
