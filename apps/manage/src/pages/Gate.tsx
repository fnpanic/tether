import { useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { invoke, type AppState } from "../lib/ipc";
import { Button, ErrorText, Label, TextField } from "../components/ui";

/** First run (create or import keys) and unlock. */
export function Gate({ state, onDone }: { state: AppState; onDone: () => void }) {
  const [mode, setMode] = useState<"unlock" | "create" | "import">(state.exists ? "unlock" : "create");
  const [pass, setPass] = useState("");
  const [pass2, setPass2] = useState("");
  const [file, setFile] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (mode === "create" && pass !== pass2) return setError("The passphrases do not match.");
    setBusy(true);
    setError(null);
    try {
      if (mode === "unlock") {
        await invoke("keys_unlock", { passphrase: pass });
        // Release indexes that expire within 15 days are re-signed (devices only warn after expiry).
        invoke("release_index_refresh").catch(() => {});
      }
      if (mode === "create") await invoke("keys_create", { passphrase: pass });
      if (mode === "import") await invoke("keys_import", { path: file, passphrase: pass });
      onDone();
    } catch (e: any) {
      setError(e.message);
    } finally {
      setBusy(false);
    }
  };

  const copy = {
    unlock: ["Unlock Tether Manage", "Enter the passphrase that protects your admin and update signing keys."],
    create: ["Set up Tether Manage", "Create the admin key (signs policies and server requests) and the update signing key. Both stay on this PC, encrypted with your passphrase."],
    import: ["Import a key backup", "Restore keys exported from Tether Manage on another PC."],
  }[mode];

  return (
    <div className="grid h-full place-items-center bg-parchment p-8">
      <form onSubmit={submit} className="animate-rise w-[440px] rounded-2xl border border-hairline bg-tile p-8">
        <h1 className="text-title font-semibold">{copy[0]}</h1>
        <p className="mt-2 text-callout text-muted">{copy[1]}</p>
        <div className="mt-6 flex flex-col gap-4">
          {mode === "import" && (
            <Label text="Backup file">
              <div className="flex gap-2">
                <TextField readOnly aria-label="Backup file" value={file} placeholder="No file chosen" />
                <Button
                  type="button"
                  onClick={async () => {
                    const f = await open({ filters: [{ name: "Tether Manage key backup", extensions: ["json", "tmkeys"] }] });
                    if (typeof f === "string") setFile(f);
                  }}
                >
                  Choose…
                </Button>
              </div>
            </Label>
          )}
          <Label text="Passphrase" hint={mode === "create" ? "At least 8 characters. There is no way to recover it: export a backup and keep the passphrase somewhere safe." : undefined}>
            <TextField type="password" autoComplete={mode === "create" ? "new-password" : "current-password"} autoFocus value={pass} onChange={(e) => setPass(e.target.value)} />
          </Label>
          {mode === "create" && (
            <Label text="Repeat passphrase">
              <TextField type="password" autoComplete="new-password" value={pass2} onChange={(e) => setPass2(e.target.value)} />
            </Label>
          )}
          <ErrorText error={error} />
          <Button variant="primary" size="lg" disabled={busy || !pass || (mode === "import" && !file)}>
            {busy ? "Working…" : mode === "unlock" ? "Unlock" : mode === "create" ? "Create keys" : "Import and unlock"}
          </Button>
          {mode === "unlock" ? null : (
            <button type="button" className="text-caption text-accent" onClick={() => setMode(mode === "create" ? "import" : state.exists ? "unlock" : "create")}>
              {mode === "create" ? "Import a backup instead" : state.exists ? "Back to unlock" : "Create new keys instead"}
            </button>
          )}
        </div>
      </form>
    </div>
  );
}
