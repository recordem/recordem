// The MCP server: Recordem's meetings, transcripts and notes as tools for agents (Claude, ChatGPT, editors…), over
// streamable HTTP at /mcp. A client of the API like any other: every tool is a call of @recordem/sdk with the caller's
// own token, so it sees exactly what the token may, and keeps nothing. An OAuth protected resource (RFC 9728): a
// client without a token is pointed at the identity provider, registers itself there and asks for one for this
// server; the identity provider lists this server among its resources and the API accepts its tokens.
import { WebStandardStreamableHTTPServerTransport } from "@modelcontextprotocol/sdk/server/webStandardStreamableHttp.js";
import { createClient } from "@recordem/sdk";
import { Hono } from "hono";
import { cors } from "hono/cors";
import { tools } from "./tools";

const env = (k: string, d?: string) => process.env[k] ?? d ?? (() => { throw new Error(`${k} is not set`); })();
const AUTH_URL = env("AUTH_URL", "http://localhost:39410");
/** The API, as this server reaches it (inside a cluster, its service). */
const API = env("MCP_API_URL", env("API_URL", "http://localhost:39411"));
/** This server's public address; the tokens it takes are issued for `${MCP_URL}/mcp`. */
const MCP_URL = env("MCP_URL", "http://localhost:39414");
const RESOURCE = `${MCP_URL}/mcp`;
const METADATA = `${MCP_URL}/.well-known/oauth-protected-resource/mcp`;

const app = new Hono();
app.use("*", cors({ origin: "*", allowHeaders: ["authorization", "content-type", "mcp-protocol-version", "mcp-session-id"], exposeHeaders: ["www-authenticate", "mcp-session-id"] }));
app.get("/healthz", (c) => c.text("ok"));

const metadata = () => Response.json({
  resource: RESOURCE,
  authorization_servers: [`${AUTH_URL}/api/auth`],
  scopes_supported: ["meetings:read", "transcripts:read", "audio:read", "offline_access"],
  bearer_methods_supported: ["header"],
  resource_name: "Recordem",
  resource_documentation: "https://github.com/recordem/recordem",
});
app.get("/.well-known/oauth-protected-resource", metadata);
app.get("/.well-known/oauth-protected-resource/mcp", metadata);

const challenge = (error?: string) => new Response(JSON.stringify({ error: error ?? "invalid_token", message: "a Bearer access token is required" }), {
  status: 401,
  headers: { "content-type": "application/json", "www-authenticate": `Bearer resource_metadata="${METADATA}"${error ? `, error="${error}"` : ""}, scope="meetings:read transcripts:read"` },
});

/** The token's audiences. Read without checking the signature: the API checks it (GET /v1/me) before this counts. */
const audiences = (jwt: string): string[] => {
  try {
    const aud = JSON.parse(Buffer.from(jwt.split(".")[1], "base64url").toString()).aud;
    return Array.isArray(aud) ? aud : [aud];
  } catch {
    return [];
  }
};

// Stateless: each request gets its own server bound to the caller's token. The token must be one issued for this
// server (not one a client got for the API, passed along), and valid: checked against the API first, so a bad one is
// a 401 the client can act on rather than a failed tool call.
app.post("/mcp", async (c) => {
  const token = c.req.header("authorization")?.match(/^Bearer (.+)$/i)?.[1];
  if (!token) return challenge();
  const api = createClient({ baseUrl: API, token: () => token });
  const me = await api.GET("/v1/me");
  if (me.response.status === 401 || (me.data && !audiences(token).includes(RESOURCE))) return challenge("invalid_token");
  if (!me.data) return c.json({ error: "server_error", message: `api: ${me.response.status}` }, 502);
  const transport = new WebStandardStreamableHTTPServerTransport({ sessionIdGenerator: undefined, enableJsonResponse: true });
  await tools(api).connect(transport);
  return transport.handleRequest(c.req.raw);
});
app.on(["GET", "DELETE"], "/mcp", (c) => c.json({ error: "method_not_allowed", message: "stateless: POST only" }, 405));

const port = Number(process.env.MCP_PORT ?? 39414);
console.log(`mcp on :${port} (api ${API}, resource ${RESOURCE})`);
export default { port, hostname: "0.0.0.0", fetch: app.fetch };
