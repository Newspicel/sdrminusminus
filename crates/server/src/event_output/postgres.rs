use std::{collections::HashMap, sync::Arc};

use rustls::ClientConfig;
use rustls_platform_verifier::BuilderVerifierExt;
use sdrmm_wire::{EventOutputTarget, valid_sql_identifier};
use tokio_postgres::{Client, Config};
use tokio_postgres_rustls::MakeRustlsConnect;

use super::{Delivery, DeliveryError, REQUEST_TIMEOUT};

pub(super) struct PostgresTarget<'a> {
    pub url: &'a str,
    pub table: &'a str,
    pub username: &'a str,
    pub password: &'a str,
}

struct Open {
    target: EventOutputTarget,
    client: Client,
}

#[derive(Default)]
pub(super) struct Connections {
    open: HashMap<String, Open>,
}

impl Connections {
    pub(super) async fn insert(
        &mut self,
        node: &str,
        target: &EventOutputTarget,
        batch: &[Delivery],
    ) -> Result<(), DeliveryError> {
        let EventOutputTarget::Postgres {
            url,
            table,
            username,
            password,
        } = target
        else {
            return Err(DeliveryError::Failed(
                "Postgres output got another service's settings".to_owned(),
            ));
        };
        let target_settings = PostgresTarget {
            url,
            table,
            username,
            password,
        };
        let client = self.client(node, target, &target_settings).await?;
        let inserted = tokio::time::timeout(REQUEST_TIMEOUT, insert_rows(client, table, batch))
            .await
            .unwrap_or_else(|_| {
                Err(DeliveryError::Failed(
                    "Postgres did not finish the insert in time".to_owned(),
                ))
            });
        if inserted.is_err() {
            self.open.remove(node);
        }
        inserted
    }

    async fn client(
        &mut self,
        node: &str,
        target: &EventOutputTarget,
        settings: &PostgresTarget<'_>,
    ) -> Result<&Client, DeliveryError> {
        let reusable = self
            .open
            .get(node)
            .is_some_and(|open| open.target == *target && !open.client.is_closed());
        if !reusable {
            let client = connect(settings).await?;
            self.open.insert(
                node.to_owned(),
                Open {
                    target: target.clone(),
                    client,
                },
            );
        }
        self.open
            .get(node)
            .map(|open| &open.client)
            .ok_or_else(|| DeliveryError::Failed("Postgres connection vanished".to_owned()))
    }
}

async fn connect(target: &PostgresTarget<'_>) -> Result<Client, DeliveryError> {
    if !valid_sql_identifier(target.table) {
        return Err(DeliveryError::Failed(format!(
            "Postgres table {:?} is not a plain lowercase name",
            target.table
        )));
    }
    let config = config(target)?;
    let (client, connection) = config
        .connect(tls()?)
        .await
        .map_err(|error| DeliveryError::Failed(format!("Postgres connect: {error}")))?;
    tokio::spawn(async move {
        if let Err(error) = connection.await {
            tracing::error!(%error, "Postgres output connection closed");
        }
    });
    client
        .batch_execute(&create_table(target.table))
        .await
        .map_err(|error| DeliveryError::Failed(format!("Postgres table setup: {error}")))?;
    Ok(client)
}

pub(super) fn config(target: &PostgresTarget<'_>) -> Result<Config, DeliveryError> {
    let mut config: Config = target
        .url
        .parse()
        .map_err(|error| DeliveryError::Failed(format!("Postgres URL: {error}")))?;
    config
        .user(target.username)
        .application_name("sdrmm")
        .connect_timeout(REQUEST_TIMEOUT);
    if !target.password.is_empty() {
        config.password(target.password);
    }
    Ok(config)
}

fn tls() -> Result<MakeRustlsConnect, DeliveryError> {
    let config = ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .and_then(BuilderVerifierExt::with_platform_verifier)
    .map_err(|error| DeliveryError::Failed(format!("Postgres TLS: {error}")))?
    .with_no_client_auth();
    Ok(MakeRustlsConnect::new(config))
}

pub(super) fn create_table(table: &str) -> String {
    format!(
        "CREATE TABLE IF NOT EXISTS {table} (
            id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
            at timestamptz NOT NULL,
            output text NOT NULL,
            kind text NOT NULL,
            device_set bigint NOT NULL,
            channel bigint NOT NULL,
            freq_hz double precision NOT NULL,
            station text,
            summary text NOT NULL,
            record jsonb NOT NULL
        );
        CREATE INDEX IF NOT EXISTS {table}_kind_at ON {table} (kind, at);"
    )
}

pub(super) fn insert_statement(table: &str) -> String {
    format!(
        "INSERT INTO {table} (at, output, kind, device_set, channel, freq_hz, station, summary, record)
         SELECT at::timestamptz, output, kind, device_set, channel, freq_hz, station, summary, record::jsonb
         FROM UNNEST($1::text[], $2::text[], $3::text[], $4::int8[], $5::int8[], $6::float8[],
                     $7::text[], $8::text[], $9::text[])
              AS rows (at, output, kind, device_set, channel, freq_hz, station, summary, record)"
    )
}

#[derive(Default)]
pub(super) struct Columns {
    pub at: Vec<String>,
    pub output: Vec<String>,
    pub kind: Vec<&'static str>,
    pub device_set: Vec<i64>,
    pub channel: Vec<i64>,
    pub freq_hz: Vec<f64>,
    pub station: Vec<Option<String>>,
    pub summary: Vec<String>,
    pub record: Vec<String>,
}

pub(super) fn columns(batch: &[Delivery]) -> Columns {
    let mut columns = Columns::default();
    for delivery in batch {
        let facts = &delivery.message.facts;
        columns.at.push(facts.at.clone());
        columns.output.push(delivery.node.clone());
        columns.kind.push(facts.kind);
        columns.device_set.push(i64::from(facts.device_set));
        columns.channel.push(i64::from(facts.channel));
        columns.freq_hz.push(facts.freq_hz);
        columns.station.push(facts.station.clone());
        columns.summary.push(facts.summary.clone());
        columns.record.push(facts.record.to_string());
    }
    columns
}

async fn insert_rows(
    client: &Client,
    table: &str,
    batch: &[Delivery],
) -> Result<(), DeliveryError> {
    let columns = columns(batch);
    let inserted = client
        .execute(
            &insert_statement(table),
            &[
                &columns.at,
                &columns.output,
                &columns.kind,
                &columns.device_set,
                &columns.channel,
                &columns.freq_hz,
                &columns.station,
                &columns.summary,
                &columns.record,
            ],
        )
        .await
        .map_err(|error| DeliveryError::Failed(format!("Postgres insert: {error}")))?;
    if inserted == batch.len() as u64 {
        Ok(())
    } else {
        Err(DeliveryError::Failed(format!(
            "Postgres stored {inserted} of {} events",
            batch.len()
        )))
    }
}
