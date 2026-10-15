// Once per database, before the identity provider's migrations (an initContainer in production): its own schema and
// role, so the api's tables and the IdP's never mix. Idempotent. Needs a superuser connection (DATABASE_ADMIN_URL);
// the role itself comes from the database operator (Zalando: users.recordem_auth).
import { Client } from "pg";

const url = process.env.DATABASE_ADMIN_URL ?? (() => { throw new Error("DATABASE_ADMIN_URL is not set"); })();
const role = process.env.AUTH_DB_ROLE ?? "recordem_auth";
const c = new Client({ connectionString: url });
await c.connect();
await c.query(`CREATE SCHEMA IF NOT EXISTS auth AUTHORIZATION ${role}`);
await c.query(`ALTER ROLE ${role} SET search_path = auth`);
await c.query(`REVOKE ALL ON SCHEMA public FROM ${role}`);
await c.end();
console.log(`schema auth ready for ${role}`);
