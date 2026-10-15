// Prints openapi.json: the routes, plus the OAuth security scheme pointing at the identity provider.
import { OpenAPIHono } from "@hono/zod-openapi";
import * as routes from "./routes";
import { SCOPES } from "./scopes";

export function document(authUrl = "http://localhost:39410", apiUrl = "http://localhost:39411") {
  const app = new OpenAPIHono();
  for (const r of Object.values(routes)) app.openapi(r, (() => {}) as never);
  app.openAPIRegistry.registerComponent("securitySchemes", "oauth2", {
    type: "oauth2",
    flows: {
      authorizationCode: {
        authorizationUrl: `${authUrl}/api/auth/oauth2/authorize`,
        tokenUrl: `${authUrl}/api/auth/oauth2/token`,
        scopes: SCOPES,
      },
      clientCredentials: { tokenUrl: `${authUrl}/api/auth/oauth2/token`, scopes: SCOPES },
    },
  });
  return app.getOpenAPI31Document({
    openapi: "3.1.0",
    info: { title: "Recordem API", version: "0.1.0", license: { name: "MIT", url: "https://opensource.org/license/mit" } },
    servers: [{ url: apiUrl }],
  });
}

if (import.meta.main) console.log(JSON.stringify(document(), null, 2));
