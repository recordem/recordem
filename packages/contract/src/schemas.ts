import { z } from "@hono/zod-openapi";

export const ErrorBody = z.object({ error: z.string(), message: z.string() }).openapi("Error");

export const Me = z.object({
  subject: z.string().openapi({ description: "The user id, or the client id for a machine token" }),
  workspace: z.string().openapi({ description: "The workspace the token acts in" }),
  scopes: z.array(z.string()),
  client: z.string().nullable().openapi({ description: "The OAuth client that obtained the token" }),
}).openapi("Me");

export const Meeting = z.object({
  id: z.string().uuid(),
  title: z.string().nullable(),
  call_key: z.string().nullable().openapi({ description: "meet:abc-defg-hij, zoom:81234567890, teams:…", example: "meet:abc-defg-hij" }),
  starts_at: z.string().datetime({ offset: true }),
  ends_at: z.string().datetime({ offset: true }).nullable(),
  created_at: z.string().datetime({ offset: true }),
}).openapi("Meeting");

export const Page = <T extends z.ZodTypeAny>(item: T) => z.object({
  items: z.array(item),
  next: z.string().nullable().openapi({ description: "Cursor for the next page; null at the end" }),
});
