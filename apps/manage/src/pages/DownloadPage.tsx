import { useState } from "react";
import { useApp } from "../App";
import { api, host, invoke, type RelayResult } from "../lib/ipc";
import { previewDoc } from "../lib/page-preview";
import { PublishSheet } from "../components/publish";
import { Button, Card, ErrorText, Label, PageHeader, TextArea, TextField, Toggle } from "../components/ui";

export function DownloadPage() {
  const { state, reload, toast } = useApp();
  const s = state.settings;
  const [title, setTitle] = useState<string>(s.page_title ?? "Remote support");
  const [intro, setIntro] = useState<string>(s.page_intro ?? "Download Tether to get help from our support team.");
  const [technician, setTechnician] = useState<boolean>(s.page_technician ?? true);
  const [customer, setCustomer] = useState<boolean>(s.page_customer ?? true);
  const [html, setHtml] = useState<string | null>(null);
  // Bumped on every Preview so the iframe is rebuilt even when the HTML did not change.
  const [shown, setShown] = useState(0);
  const [reviewing, setReviewing] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const render = async () => {
    setError(null);
    try {
      const h = await invoke<string>("page_render", { title, intro, technician, customer });
      setHtml(h);
      setShown((n) => n + 1);
      return h;
    } catch (e: any) {
      setError(e.message);
      return null;
    }
  };

  const publish = async (): Promise<RelayResult[]> => {
    const h = await invoke<string>("page_render", { title, intro, technician, customer });
    setHtml(h);
    const r = await invoke<RelayResult[]>("page_publish", { html: h });
    await api.saveSettings({ ...s, page_title: title, page_intro: intro, page_technician: technician, page_customer: customer });
    await reload();
    toast("Download page published");
    return r;
  };

  return (
    <>
      <PageHeader title="Download page" sub={`Served at ${state.servers.map((x) => `${host(x)}/`).join(" and ")} — the current stable release for Windows and macOS; visitors see their own OS first.`}>
        <Button onClick={render}>Preview</Button>
        <Button variant="primary" disabled={!title.trim() || (!technician && !customer)} onClick={() => setReviewing(true)}>
          Publish…
        </Button>
      </PageHeader>
      <div className="grid grid-cols-[340px_minmax(0,1fr)] items-start gap-6">
        <Card className="flex flex-col gap-4">
          <Label text="Title">
            <TextField value={title} onChange={(e) => setTitle(e.target.value)} />
          </Label>
          <Label text="Introduction" hint="Separate paragraphs with an empty line.">
            <TextArea rows={6} value={intro} onChange={(e) => setIntro(e.target.value)} />
          </Label>
          <div className="flex items-center justify-between">
            <span className="text-caption">Technician app (installer / disk image)</span>
            <Toggle label="Offer technician installer" checked={technician} onChange={setTechnician} />
          </div>
          <div className="flex items-center justify-between">
            <span className="text-caption">Customer module (TetherQS)</span>
            <Toggle label="Offer customer module" checked={customer} onChange={setCustomer} />
          </div>
          <p className="text-footnote text-muted">Each app gets one card with one Download button. Signatures and the macOS updater archive are folded under "More files" in the same card.</p>
          <ErrorText error={error} />
        </Card>
        <div className="h-[560px] overflow-hidden rounded-2xl border border-hairline bg-tile">
          {html ? (
            <iframe key={shown} title="Download page preview" sandbox="" srcDoc={previewDoc(html)} className="h-full w-full" />
          ) : (
            <p className="grid h-full place-items-center text-caption text-muted">Preview appears here.</p>
          )}
        </div>
      </div>
      {reviewing && (
        <PublishSheet title="Publish the download page" action="Publish page" run={publish} onClose={() => setReviewing(false)}>
          <dl className="grid grid-cols-[140px_1fr] gap-x-4 gap-y-1.5 text-caption">
            <dt className="text-muted">Title</dt>
            <dd className="font-semibold">{title}</dd>
            <dt className="text-muted">Offers</dt>
            <dd>{[technician && "Technician app", customer && "Customer module"].filter(Boolean).join(" and ")}, Windows and macOS</dd>
          </dl>
          <p className="mt-3 text-caption text-muted">
            The page is built from the first relay's stable latest.json and uploaded to every relay. Its links are relative, so each relay serves its own files: publish releases to every relay first (Releases shows what is missing).
          </p>
        </PublishSheet>
      )}
    </>
  );
}
