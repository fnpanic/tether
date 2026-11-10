/** The download page as shown in the sandboxed preview iframe. Its links are relative to the relay;
 *  inside the srcdoc they resolve against Tether Manage itself and a click would replace the preview
 *  with a blank app page. `target="_blank"` turns every link into a popup, which `sandbox=""` blocks. */
export const previewDoc = (html: string) => html.replace(/<head(\s[^>]*)?>/i, '$&<base target="_blank">');
