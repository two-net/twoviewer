//! Helpers shared by the integration tests.

use sqlx::{Connection, PgConnection};
use testcontainers_modules::{
    postgres::Postgres,
    testcontainers::{ContainerAsync, ImageExt, runners::AsyncRunner},
};

/// A PostgreSQL server for one test, removed when dropped, and the URL of its database.
pub async fn postgres() -> (ContainerAsync<Postgres>, String) {
    let container = Postgres::default()
        .with_tag("18-alpine")
        .start()
        .await
        .expect("start a PostgreSQL container (is Docker running?)");
    let url = format!(
        "postgres://postgres:postgres@{}:{}/postgres",
        container.get_host().await.unwrap(),
        container.get_host_port_ipv4(5432).await.unwrap()
    );
    (container, url)
}

pub async fn execute(url: &str, sql: &'static str) {
    let mut conn = PgConnection::connect(url).await.unwrap();
    sqlx::raw_sql(sql).execute(&mut conn).await.unwrap();
}
