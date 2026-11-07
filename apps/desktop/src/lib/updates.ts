// Update status text (Settings › About, Diagnostics). Kept out of UpdateCard.tsx for unit tests.
import type { UpdateStatus } from "../components/UpdateCard";

/** One line for Settings and Diagnostics: where the update stands. */
export function updateLine(s: UpdateStatus): string {
  const parts = [s.version ? `Version ${s.version} is available.` : `Tether ${s.current} is up to date.`];
  if (s.rollout) parts.push(`${s.rollout.version} is rolling out (${s.rollout.percent} % of devices so far).`);
  if (s.phase === "staged") parts.push("Downloaded and verified.");
  if (s.phase === "halted") parts.push("The release was halted by your administrator.");
  if (s.stale) parts.push("Update information is out of date.");
  return parts.join(" ");
}
