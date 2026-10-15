import { drizzle } from "drizzle-orm/postgres-js";
import postgres from "postgres";
import * as schema from "./schema";

export * from "./schema";
export const connect = (url: string) => drizzle(postgres(url), { schema });
export type Db = ReturnType<typeof connect>;
