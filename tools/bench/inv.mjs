// inv.mjs <port> <command> [json-args]: invoke a Tauri command in the viewer's session page; "$KEY" in args = session key.
import { createRequire } from "module";
const require = createRequire(new URL("../../apps/desktop/package.json", import.meta.url));
const { chromium } = require("playwright-core");
const [port, cmd, args = "{}"] = process.argv.slice(2);
const b = await chromium.connectOverCDP(`http://127.0.0.1:${port}`);
const p = b.contexts().flatMap((c) => c.pages()).find((x) => x.url().includes("/session/"));
const key = p.url().match(/session\/([0-9a-f]{32})/)[1];
const a = JSON.parse(args.replaceAll("$KEY", key));
console.log(cmd, await p.evaluate(([c, a]) => window.__TAURI_INTERNALS__.invoke(c, a).then((r) => "ok " + JSON.stringify(r ?? null).slice(0, 200), (e) => "ERR " + e), [cmd, a]));
await b.close();
