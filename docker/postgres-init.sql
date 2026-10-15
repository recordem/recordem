-- The identity provider keeps its tables in schema "auth", under its own role; the api never reads them.
CREATE ROLE recordem_auth LOGIN PASSWORD 'recordem_auth';
CREATE SCHEMA auth AUTHORIZATION recordem_auth;
ALTER ROLE recordem_auth SET search_path = auth;
REVOKE ALL ON SCHEMA public FROM recordem_auth;
