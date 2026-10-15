// Every route of the API. apps/api implements them; nothing else is served under /v1.
import { createRoute, z } from "@hono/zod-openapi";
import { ErrorBody, Me, Meeting, Page } from "./schemas";
import type { Scope } from "./scopes";

const json = <T extends z.ZodTypeAny>(schema: T, description: string) => ({ content: { "application/json": { schema } }, description });
const errors = { 401: json(ErrorBody, "Missing or invalid token"), 403: json(ErrorBody, "Token lacks a required scope") };
/** Scopes a route requires, read by the API's auth middleware. */
const needs = (...scopes: Scope[]) => ({ security: [{ oauth2: scopes }], "x-scopes": scopes });

export const getMe = createRoute({
  method: "get", path: "/v1/me", operationId: "getMe", tags: ["identity"],
  summary: "Who the token is", ...needs(),
  responses: { 200: json(Me, "The token's subject, workspace and scopes"), ...errors },
});

export const listMeetings = createRoute({
  method: "get", path: "/v1/meetings", operationId: "listMeetings", tags: ["meetings"],
  summary: "Meetings of the workspace, newest first", ...needs("meetings:read"),
  request: {
    query: z.object({
      limit: z.coerce.number().int().min(1).max(200).default(50),
      cursor: z.string().optional().openapi({ description: "`next` from the previous page" }),
    }),
  },
  responses: { 200: json(Page(Meeting), "A page of meetings"), ...errors },
});
