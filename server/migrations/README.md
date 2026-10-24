# Database migrations

The data model is defined in code as SeaORM entities (`server/src/db`), and every entity is registered in `db::schema_builder()`. The SQL files in this directory are generated from that model. They are embedded in the server, which applies pending ones at startup before it serves traffic (§5.1 of the SRS).

## Changing the schema

1. Change or add entities, and register new ones in `db::schema_builder()`.
2. Run `cargo xtask new-migration <snake_case_name>`. It writes `<UTC timestamp>_<name>.sql` with the DDL needed to bring the migrations in line with the model.
3. If it reports remaining differences, the change is one that schema sync cannot generate: changed types or nullability, renames, drops. Write those steps by hand at the end of the file, then run `cargo xtask check-migrations` until it passes.
4. Review the SQL and commit it together with the entity change.

Both commands start a throwaway PostgreSQL container, as do the tests in `server/tests`, so they need Docker. testcontainers does not follow Docker contexts. With Colima or another non-default socket, run:

```sh
export DOCKER_HOST=$(docker context inspect -f '{{.Endpoints.docker.Host}}')
```

## Rules

- **Never edit an applied migration.** The server stores each file's checksum and refuses to start if a file has changed. Correct mistakes with a new migration (DB-12).
- **Forward only.** There are no down migrations. Spread destructive changes, such as dropping or renaming a column or table, over two releases: first stop using it, then remove it. This keeps the previous version working during a deployment (DB-12).
- **Hand-written SQL only for what the ORM cannot express** (DB-04), such as the trigger that rejects updates to audit events. Put it below the marker comment, with a comment saying why.
- **One transaction per migration.** The runner wraps each file in a transaction (DB-06), so do not use `BEGIN`/`COMMIT` or statements that cannot run in a transaction, such as `CREATE INDEX CONCURRENTLY`.
- **No data.** Migrations create no demo data or default accounts (DB-08).
- **No manual schema changes** in any environment (DB-03).
