import { useEffect, useRef, useState } from "react";
import { ask, save } from "@tauri-apps/plugin-dialog";
import { Trash2 } from "lucide-react";
import { useApp } from "../App";
import { api, host, inTauri, invoke, type RelayResult } from "../lib/ipc";
import { previewDoc } from "../lib/page-preview";
import { buildPage, MAX_PAGE, pageHtml, reference, type CustomApp, type Filled, type PageSettings } from "../lib/page-template";
import starter from "../../../../docs/examples/download-template.html?raw";
import { PublishSheet } from "../components/publish";
import { Button, Card, CopyField, ErrorText, IconButton, Label, PageHeader, Segmented, Select, TextArea, TextField, Toggle } from "../components/ui";

type Design = "standard" | "custom";

/** `1730000000000-my_page.html` → `my_page.html · <local save time>`. */
const templateLabel = (file: string) => {
  const [ms, ...rest] = file.split("-");
  return `${rest.join("-")} · ${new Date(Number(ms)).toLocaleString()}`;
};

export function DownloadPage() {
  const { state, reload, toast } = useApp();
  const s = state.settings;
  const [title, setTitle] = useState<string>(s.page_title ?? "Remote support");
  const [intro, setIntro] = useState<string>(s.page_intro ?? "Download Tether to get help from our support team.");
  const [technician, setTechnician] = useState<boolean>(s.page_technician ?? true);
  const [customer, setCustomer] = useState<boolean>(s.page_customer ?? true);
  // Standard (the built-in page) unless a custom design was published; switching back keeps the templates.
  const [design, setDesign] = useState<Design>(s.page_design === "custom" ? "custom" : "standard");
  const [template, setTemplate] = useState<string | null>(s.page_template ?? null);
  const [allowExternal, setAllowExternal] = useState<boolean>(s.page_allow_external ?? false);
  const [templates, setTemplates] = useState<{ file: string; size: number }[]>([]);
  const [custom, setCustom] = useState<CustomApp[]>([]);
  const [html, setHtml] = useState<string | null>(null);
  const [filled, setFilled] = useState<Filled | null>(null);
  // Bumped on every Preview so the iframe is rebuilt even when the HTML did not change.
  const [shown, setShown] = useState(0);
  const [reviewing, setReviewing] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const picker = useRef<HTMLInputElement>(null);

  const listTemplates = () => invoke<{ file: string; size: number }[]>("page_template_list").then(setTemplates);
  useEffect(() => {
    listTemplates().catch(() => {});
  }, []);
  useEffect(() => {
    if (design === "custom" && !custom.length) invoke<CustomApp[]>("custom_list").then((c) => setCustom(c.filter((x) => x.status === "active")), () => {});
  }, [design]);

  const settings: PageSettings = { ...s, page_title: title, page_intro: intro, page_technician: technician, page_customer: customer, page_design: design, page_template: template, page_allow_external: allowExternal };
  const clear = () => {
    setHtml(null);
    setFilled(null);
  };

  const render = async () => {
    setError(null);
    setFilled(null);
    try {
      const f = await buildPage(settings, state.servers, invoke);
      setFilled(design === "custom" ? f : null);
      setHtml(f.html || null);
      setShown((n) => n + 1);
    } catch (e: any) {
      setError(e.message);
    }
  };

  const publish = async (skip: string[]): Promise<RelayResult[]> => {
    // Relays left out are not read either (as after a release, Releases.tsx).
    const h = await pageHtml(settings, state.servers, invoke, skip);
    setHtml(h);
    const r = await invoke<RelayResult[]>("page_publish", { html: h, skip });
    // Saved also when only some relays took it: the page they serve is rebuilt from these settings.
    await api.saveSettings(settings);
    await reload();
    const done = r.filter((x) => x.ok).length;
    toast(done === r.length ? "Download page published" : `Download page published on ${done} of ${r.length} relays only`);
    return r;
  };

  // The published template is refused by the backend (the page is rebuilt from it after releases).
  const remove = async (file: string) => {
    const sure = inTauri ? await ask(`Delete the template ${templateLabel(file)}? This cannot be undone.`, { title: "Delete template", kind: "warning" }) : true;
    if (!sure) return;
    setError(null);
    try {
      await invoke("page_template_delete", { file });
      await listTemplates();
      if (template === file) {
        setTemplate(null);
        clear();
      }
      toast("Template deleted");
    } catch (e: any) {
      setError(e.message);
    }
  };
  const published = s.page_design === "custom" ? s.page_template : null;

  const upload = async (file: File | undefined) => {
    if (!file) return;
    setError(null);
    try {
      if (file.size > MAX_PAGE) throw new Error(`${file.name} is larger than ${MAX_PAGE / 1024} KiB (the relay's page limit).`);
      const saved = await invoke<string>("page_template_save", { name: file.name, html: await file.text() });
      await listTemplates();
      setTemplate(saved);
      clear();
    } catch (e: any) {
      setError(e.message);
    } finally {
      if (picker.current) picker.current.value = "";
    }
  };

  const downloadStarter = async () => {
    const name = "tether-download-template.html";
    if (!inTauri) {
      Object.assign(document.createElement("a"), { href: URL.createObjectURL(new Blob([starter], { type: "text/html" })), download: name }).click();
      return;
    }
    const p = await save({ defaultPath: name, filters: [{ name: "HTML", extensions: ["html"] }] });
    if (p) await invoke("text_save", { path: p, text: starter }).then(() => toast("Saved"), (e: any) => setError(e.message));
  };

  const blocked = design === "custom" ? !template || !!filled?.errors.length : !technician && !customer;
  return (
    <>
      <PageHeader title="Download page" sub={`Served at ${state.servers.map((x) => `${host(x)}/`).join(" and ")} — the current stable release for Windows and macOS; visitors see their own OS first.`}>
        <Button onClick={render}>Preview</Button>
        <Button variant="primary" disabled={!title.trim() || blocked} onClick={() => setReviewing(true)}>
          Publish…
        </Button>
      </PageHeader>
      <div className="grid grid-cols-[340px_minmax(0,1fr)] items-start gap-6">
        <Card className="flex flex-col gap-4">
          <Label text="Design" hint={design === "custom" ? "Your own HTML; Tether Manage fills in the links and release details. Standard stays available unchanged." : "The built-in page. Custom HTML uses your own file."}>
            <Segmented<Design>
              label="Design"
              value={design}
              onChange={(d) => {
                setDesign(d);
                clear();
              }}
              options={[
                { value: "standard", label: "Standard (built-in)" },
                { value: "custom", label: "Custom HTML" },
              ]}
            />
          </Label>
          <Label text="Title" hint={design === "custom" ? "{{tether.title}} in the template." : undefined}>
            <TextField value={title} onChange={(e) => setTitle(e.target.value)} />
          </Label>
          <Label text="Introduction" hint={design === "custom" ? "{{tether.intro}} in the template (plain text)." : "Separate paragraphs with an empty line."}>
            <TextArea rows={design === "custom" ? 3 : 6} value={intro} onChange={(e) => setIntro(e.target.value)} />
          </Label>
          {design === "standard" ? (
            <>
              <div className="flex items-center justify-between">
                <span className="text-caption">Technician app (installer / disk image)</span>
                <Toggle label="Offer technician installer" checked={technician} onChange={setTechnician} />
              </div>
              <div className="flex items-center justify-between">
                <span className="text-caption">Customer module (TetherQS)</span>
                <Toggle label="Offer customer module" checked={customer} onChange={setCustomer} />
              </div>
              <p className="text-footnote text-muted">Each app gets one card with one Download button. Signatures and the macOS updater archive are folded under "More files" in the same card.</p>
            </>
          ) : (
            <>
              <Label text="Template" hint="Every file you choose is kept; pick an earlier one to go back, or delete versions you no longer need.">
                <div className="flex items-center gap-2">
                <Select
                  aria-label="Template"
                  className="min-w-0 flex-1"
                  value={template ?? ""}
                  onChange={(e) => {
                    setTemplate(e.target.value || null);
                    clear();
                  }}
                >
                  <option value="">No template chosen</option>
                  {templates.map((t) => (
                    <option key={t.file} value={t.file}>
                      {templateLabel(t.file)}
                      {t.file === published ? " · published" : ""}
                    </option>
                  ))}
                </Select>
                {template && (
                  <IconButton
                    label={template === published ? "The published page uses this template" : "Delete this template"}
                    className="disabled:pointer-events-none disabled:opacity-30"
                    disabled={template === published}
                    onClick={() => remove(template)}
                  >
                    <Trash2 size={16} />
                  </IconButton>
                )}
                </div>
              </Label>
              <div className="flex flex-wrap gap-2">
                <Button size="sm" onClick={() => picker.current?.click()}>
                  Choose HTML file…
                </Button>
                <Button size="sm" onClick={downloadStarter}>
                  Download starter template
                </Button>
                <input ref={picker} type="file" accept=".html,.htm,text/html" className="hidden" onChange={(e) => upload(e.target.files?.[0])} />
              </div>
              <div className="flex items-center justify-between gap-3">
                <span className="text-caption">Allow resources from other sites (fonts, images, scripts)</span>
                <Toggle label="Allow external resources" checked={allowExternal} onChange={setAllowExternal} />
              </div>
              {filled && (
                <div className="flex flex-col gap-1.5 text-footnote">
                  {filled.errors.map((x) => (
                    <p key={x} className="text-danger">
                      {x}
                    </p>
                  ))}
                  {filled.warnings.map((x) => (
                    <p key={x} className="text-warn">
                      {x}
                    </p>
                  ))}
                  {!filled.errors.length && <p className="text-muted">Ready to publish · {filled.used.length} placeholders used.</p>}
                </div>
              )}
            </>
          )}
          <ErrorText error={error} />
        </Card>
        <div className="h-[560px] overflow-hidden rounded-2xl border border-hairline bg-tile">
          {html ? (
            <iframe key={shown} title="Download page preview" sandbox="" srcDoc={previewDoc(html)} className="h-full w-full" />
          ) : (
            <p className="grid h-full place-items-center text-caption text-muted">{filled?.errors.length ? "Fix the errors on the left, then Preview." : "Preview appears here."}</p>
          )}
        </div>
      </div>
      {design === "custom" && (
        <Card className="mt-6">
          <details>
            <summary className="cursor-pointer text-headline font-semibold">Placeholder reference</summary>
            <p className="mt-2 text-footnote text-muted">
              Values are HTML-escaped. Use them in text or quoted attributes; only .url placeholders go in href or src, at the start. Wrap a release in its section so the page still works while it has none (docs/download-page.md).
            </p>
            <div className="mt-3 grid grid-cols-[minmax(0,420px)_1fr] items-center gap-x-4 gap-y-1.5">
              {reference(custom).map((r) => (
                <div key={r.name} className="contents">
                  <CopyField value={r.name.replace("…", "")} label={r.name} />
                  <span className="text-footnote text-muted">{r.desc}</span>
                </div>
              ))}
            </div>
          </details>
        </Card>
      )}
      {reviewing && (
        <PublishSheet title="Publish the download page" action="Publish page" run={publish} onClose={() => setReviewing(false)}>
          <dl className="grid grid-cols-[140px_1fr] gap-x-4 gap-y-1.5 text-caption">
            <dt className="text-muted">Title</dt>
            <dd className="font-semibold">{title}</dd>
            <dt className="text-muted">Design</dt>
            <dd>{design === "custom" ? `Custom HTML: ${template ? templateLabel(template) : "none"}` : "Standard (built-in)"}</dd>
            {design === "standard" && (
              <>
                <dt className="text-muted">Offers</dt>
                <dd>{[technician && "Technician app", customer && "Customer module"].filter(Boolean).join(" and ")}, Windows and macOS</dd>
              </>
            )}
          </dl>
          <p className="mt-3 text-caption text-muted">
            The page is built from the first relay's stable release data and uploaded to every relay. Its links are relative, so each relay serves its own files: publish releases to every relay first (Releases shows what is missing). A stable release publish rebuilds the page with this design.
          </p>
        </PublishSheet>
      )}
    </>
  );
}
