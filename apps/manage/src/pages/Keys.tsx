import { useState } from "react";
import { ask, open, save } from "@tauri-apps/plugin-dialog";
import { useApp } from "../App";
import { invoke } from "../lib/ipc";
import { Button, Card, CopyField, ErrorText, Label, PageHeader, TextField } from "../components/ui";
import { ServerKey } from "../components/server-key";

export function Keys() {
  const { state, reload, toast } = useApp();
  const [importPass, setImportPass] = useState("");
  const [error, setError] = useState<string | null>(null);

  const run = async (f: () => Promise<unknown>, ok: string) => {
    setError(null);
    try {
      await f();
      toast(ok);
      await reload();
    } catch (e: any) {
      setError(e.message);
    }
  };

  return (
    <>
      <PageHeader title="Keys" sub="Private keys never leave this PC. Only the public halves below are meant to be copied. Relays are set on Status and relays." />
      <div className="flex flex-col gap-6">
        <Card>
          <h2 className="text-headline font-semibold">Admin public key</h2>
          <p className="mb-3 mt-1 text-caption text-muted">
            Put this into the server's <code className="font-mono">ADMIN_PUB_KEY</code> (/etc/tether/api.env) and into <code className="font-mono">TETHER_ADMIN_PUB_KEY</code> when building the apps.
          </p>
          <CopyField label="admin public key" value={state.admin_pub ?? ""} />
          <h2 className="mt-6 text-headline font-semibold">Update signing public key</h2>
          <p className="mb-3 mt-1 text-caption text-muted">
            For the build variable <code className="font-mono">TETHER_UPDATER_PUB_KEY</code> (technician app and customer module).
          </p>
          <CopyField label="update public key" value={state.updater_pub ?? ""} />
        </Card>

        <ServerKey />

        <Card>
          <h2 className="text-headline font-semibold">Backup</h2>
          <p className="mt-1 text-caption text-muted">
            The backup is encrypted with your current passphrase. Without it, no one can use the keys and you cannot publish policies or updates. It also holds server keys generated here: export it again after generating one. Keys are stored in {state.data_dir}.
          </p>
          <div className="mt-4 flex flex-wrap items-end gap-2">
            <Button
              onClick={async () => {
                const p = await save({ defaultPath: "tether-manage-keys.tmkeys", filters: [{ name: "Tether Manage key backup", extensions: ["tmkeys"] }] });
                if (p) run(() => invoke("keys_export", { path: p }), "Backup saved");
              }}
            >
              Export backup…
            </Button>
            <div className="ml-auto flex items-end gap-2">
              <Label text="Import: passphrase of the backup">
                <TextField type="password" autoComplete="off" value={importPass} onChange={(e) => setImportPass(e.target.value)} />
              </Label>
              <Button
                variant="danger"
                disabled={!importPass}
                onClick={async () => {
                  const f = await open({ filters: [{ name: "Tether Manage key backup", extensions: ["tmkeys", "json"] }] });
                  if (typeof f !== "string") return;
                  if (!(await ask("Importing replaces the keys on this PC. Devices only trust the new keys if their public keys are deployed. Continue?", { title: "Replace keys", kind: "warning" }))) return;
                  run(() => invoke("keys_import", { path: f, passphrase: importPass }).then(() => setImportPass("")), "Keys imported");
                }}
              >
                Import backup…
              </Button>
            </div>
          </div>
        </Card>
        <ErrorText error={error} />
      </div>
    </>
  );
}
