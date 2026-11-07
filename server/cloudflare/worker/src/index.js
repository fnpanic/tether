// Runs only when no static asset matches (see wrangler.toml): downloads themselves are served by
// Workers Static Assets without invoking this script. Read-only: everything else is 404 or 405.
export default {
  async fetch(request) {
    return miss(request);
  },
};

export function miss(request) {
  const headers = { "Content-Type": "text/plain; charset=utf-8", "Cache-Control": "no-store", "X-Content-Type-Options": "nosniff" };
  if (request.method !== "GET" && request.method !== "HEAD") {
    return new Response("method not allowed\n", { status: 405, headers: { ...headers, Allow: "GET, HEAD" } });
  }
  return new Response(request.method === "HEAD" ? null : "not found\n", { status: 404, headers });
}
