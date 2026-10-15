// The api database. Workspaces and users live in the identity provider: here they are only ids (text, as issued).
import { index, pgTable, text, timestamp, uuid } from "drizzle-orm/pg-core";

export const meetings = pgTable("meetings", {
  id: uuid("id").primaryKey().defaultRandom(),
  workspaceId: text("workspace_id").notNull(),
  title: text("title"),
  callKey: text("call_key"),                 // meet:abc-defg-hij, zoom:…, teams:… — how recordings and events meet
  startsAt: timestamp("starts_at", { withTimezone: true }).notNull(),
  endsAt: timestamp("ends_at", { withTimezone: true }),
  createdBy: text("created_by").notNull(),   // subject of the token that created it
  createdAt: timestamp("created_at", { withTimezone: true }).notNull().defaultNow(),
}, (t) => [
  index("meetings_workspace_starts").on(t.workspaceId, t.startsAt.desc(), t.id.desc()),
  index("meetings_workspace_call").on(t.workspaceId, t.callKey),
]);
