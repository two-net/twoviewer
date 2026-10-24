# Database migrations

The data model is defined in code as SeaORM entities (`server/src/db`), and every entity is registered in `db::schema_builder()`. The SQL files in this directory are generated from that model. They are embedded in the server, which applies pending ones at startup before it serves traffic (§5.1 of the SRS), and reports ready only after that ([`../README.md`](../README.md)).

## Changing the schema

1. Change or add entities, and register new ones in `db::schema_builder()`.
2. Run `cargo xtask new-migration <snake_case_name>`. It writes `<UTC timestamp>_<name>.sql` with the DDL needed to bring the migrations in line with the model.
3. If it reports remaining differences, the change is one that schema sync cannot generate: changed types or nullability, renames, drops. Write those steps by hand at the end of the file, then run `cargo xtask check-migrations` until it passes.
4. Review the SQL and commit it together with the entity change.

Both commands start a throwaway PostgreSQL container, as do the tests in `server/tests`, so they need Docker. testcontainers does not follow Docker contexts. With Colima or another non-default socket, run:

```sh
export DOCKER_HOST=$(docker context inspect -f '{{.Endpoints.docker.Host}}')
```

## Checks

CI checks every change (MNT-05, [`../../README.md`](../../README.md#checks)):

- `cargo test -p twoviewer-server --test migrate` applies all migrations to an empty database, and upgrades from the previous release. That test applies the migrations of the newest `v<major>.<minor>.<patch>` tag before the current commit, then the current ones. It fails if a released migration was edited, renamed or removed, or if a new one sorts before it. Before the first release, it has nothing to upgrade from and passes.
- `cargo xtask check-migrations` fails if the migrations and the code model differ.

## Entity conventions

Every entity follows these rules (§5.1 of the SRS). `cargo test -p twoviewer-server --test schema_conventions` checks the migrated schema against them. The `convention_example` entity in that test is a complete reference.

- **Random UUID keys** (DB-15). Identifiers end up in URLs, so they must be hard to guess: no serial, identity or integer keys. The database generates a random UUIDv4. Leave `id` unset on insert, and `ActiveModel::insert` returns it:

  ```rust
  #[sea_orm(primary_key, auto_increment = false, default_expr = "Expr::cust(\"gen_random_uuid()\")")]
  pub id: Uuid,
  ```

  A table keyed by its parent's ID (one-to-one) has no default. Its key is a foreign key instead.
- **UTC timestamps** (DB-14). Every point in time is a `DateTimeUtc` (`timestamp with time zone`), never `DateTime` or `NaiveDateTime`. Database sessions run in UTC.
- **Integrity in the database** (DB-13), declared in the entity:
  - A field is an `Option` only when its absence means something, such as `revoked_at`. Everything else is `NOT NULL`.
  - A reference is a foreign key: `#[sea_orm(belongs_to, from = "user_id", to = "id")]`. Index its column with `#[sea_orm(indexed)]`, so deleting the referenced row doesn't scan the table.
  - Natural keys are `unique` (one column) or `unique_key = "<name>"` (several).
  - Email addresses are unique regardless of case: store them as `db::EmailAddress`, which lowercases, and let the database reject anything else:

    ```rust
    #[sea_orm(unique, extra = "CHECK (email = lower(email))")]
    pub email: EmailAddress,
    ```
- **No partitioning** (DB-11): no `PARTITION BY`, table inheritance, foreign tables or sharding. Large tables, such as audit events, stay fast through indexes and batched retention deletes (RET-02).
- **Data access through the ORM** (DB-16). Query with SeaORM. sqlx is used directly only by the migration runner, which needs a session-level advisory lock. Write SQL by hand only where the ORM is too slow, as a static string with bind parameters. Never `format!` values into SQL. Clippy rejects the APIs that run SQL built at runtime (`clippy.toml`). Where one is unavoidable, add `#[expect(clippy::disallowed_methods, reason = "...")]` on that statement.

## Rules

- **Never edit an applied migration.** The server stores each file's checksum and refuses to start if a file has changed. Correct mistakes with a new migration (DB-12).
- **Forward only.** There are no down migrations. Spread destructive changes, such as dropping or renaming a column or table, over two releases: first stop using it, then remove it. This keeps the previous version working during a deployment (DB-12).
- **Hand-written SQL only for what the ORM cannot express** (DB-04), such as the trigger that rejects updates to audit events. Put it below the marker comment, with a comment saying why.
- **One transaction per migration.** The runner wraps each file in a transaction (DB-06), so do not use `BEGIN`/`COMMIT` or statements that cannot run in a transaction, such as `CREATE INDEX CONCURRENTLY`.
- **No data.** Migrations create no demo data or default accounts (DB-08).
- **No manual schema changes** in any environment (DB-03).
