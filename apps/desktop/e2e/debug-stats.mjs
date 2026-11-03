// Ad-hoc: open the session statistics overlay in a running viewer (CDP :9223) and print it.
import { chromium } from "playwright-core";
const b = await chromium.connectOverCDP("http://127.0.0.1:9223");
const p = b.contexts().flatMap((c) => c.pages()).find((x) => x.url().includes("/session/"));
await p.evaluate(() => document.querySelector('[aria-label="Statistics"]')?.click());
for (let i = 0; i < 4; i++) {
  await new Promise((r) => setTimeout(r, 2000));
  const txt = await p.evaluate(() => {
    const o = [...document.querySelectorAll("div")].find((d) => d.textContent?.startsWith("fps"));
    return o ? o.innerText.replace(/\n/g, " ") : "overlay not found";
  });
  console.log(txt);
}
await b.close();
