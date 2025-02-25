mod generated;

// useful re-exports if types are needed in other crates
pub use prost;
pub use prost_types;
pub use tonic;

#[cfg(any(test, feature = "test-utils"))]
pub mod test_utils;

#[cfg(feature = "provisioner")]
pub mod provisioner {
    use std::fmt::Display;

    use shuttle_common::{
        database::{self, AwsRdsEngine, SharedEngine},
        DatabaseInfo,
    };

    pub use super::generated::provisioner::*;

    impl From<DatabaseResponse> for DatabaseInfo {
        fn from(response: DatabaseResponse) -> Self {
            DatabaseInfo::new(
                response.engine,
                response.username,
                response.password,
                response.database_name,
                response.port,
                response.address_private,
                response.address_public,
            )
        }
    }

    impl From<database::Type> for database_request::DbType {
        fn from(db_type: database::Type) -> Self {
            match db_type {
                database::Type::Shared(engine) => {
                    let engine = match engine {
                        SharedEngine::Postgres => shared::Engine::Postgres(String::new()),
                        SharedEngine::MongoDb => shared::Engine::Mongodb(String::new()),
                    };
                    database_request::DbType::Shared(Shared {
                        engine: Some(engine),
                    })
                }
                database::Type::AwsRds(engine) => {
                    let config = RdsConfig {};
                    let engine = match engine {
                        AwsRdsEngine::Postgres => aws_rds::Engine::Postgres(config),
                        AwsRdsEngine::MariaDB => aws_rds::Engine::Mariadb(config),
                        AwsRdsEngine::MySql => aws_rds::Engine::Mysql(config),
                    };
                    database_request::DbType::AwsRds(AwsRds {
                        engine: Some(engine),
                    })
                }
            }
        }
    }

    impl From<database_request::DbType> for Option<database::Type> {
        fn from(db_type: database_request::DbType) -> Self {
            match db_type {
                database_request::DbType::Shared(Shared {
                    engine: Some(engine),
                }) => match engine {
                    shared::Engine::Postgres(_) => {
                        Some(database::Type::Shared(SharedEngine::Postgres))
                    }
                    shared::Engine::Mongodb(_) => {
                        Some(database::Type::Shared(SharedEngine::MongoDb))
                    }
                },
                database_request::DbType::AwsRds(AwsRds {
                    engine: Some(engine),
                }) => match engine {
                    aws_rds::Engine::Postgres(_) => {
                        Some(database::Type::AwsRds(AwsRdsEngine::Postgres))
                    }
                    aws_rds::Engine::Mysql(_) => Some(database::Type::AwsRds(AwsRdsEngine::MySql)),
                    aws_rds::Engine::Mariadb(_) => {
                        Some(database::Type::AwsRds(AwsRdsEngine::MariaDB))
                    }
                },
                database_request::DbType::Shared(Shared { engine: None })
                | database_request::DbType::AwsRds(AwsRds { engine: None }) => None,
            }
        }
    }

    impl Display for aws_rds::Engine {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                Self::Mariadb(_) => write!(f, "mariadb"),
                Self::Mysql(_) => write!(f, "mysql"),
                Self::Postgres(_) => write!(f, "postgres"),
            }
        }
    }
}

#[cfg(feature = "runtime")]
pub mod runtime {
    pub use super::generated::runtime::*;
}

#[cfg(feature = "resource-recorder")]
pub mod resource_recorder {
    use anyhow::Context;
    use async_trait::async_trait;
    use http::{header::AUTHORIZATION, Uri};
    use shuttle_common::backends::client::{self, ResourceDal};
    use std::str::FromStr;

    use self::resource_recorder_client::ResourceRecorderClient;

    pub use super::generated::resource_recorder::*;

    impl TryFrom<record_request::Resource> for shuttle_common::resource::Response {
        type Error = anyhow::Error;

        fn try_from(resource: record_request::Resource) -> Result<Self, Self::Error> {
            let r#type = shuttle_common::resource::Type::from_str(resource.r#type.as_str())
                .map_err(anyhow::Error::msg)
                .context("resource type should have a valid resource string")?;
            let response = shuttle_common::resource::Response {
                r#type,
                config: serde_json::from_slice(&resource.config)
                    .context(format!("{} resource config should be valid JSON", r#type))?,
                data: serde_json::from_slice(&resource.data)
                    .context(format!("{} resource data should be valid JSON", r#type))?,
            };

            Ok(response)
        }
    }

    impl TryFrom<Resource> for shuttle_common::resource::Response {
        type Error = anyhow::Error;

        fn try_from(resource: Resource) -> Result<Self, Self::Error> {
            let r#type = shuttle_common::resource::Type::from_str(resource.r#type.as_str())
                .map_err(anyhow::Error::msg)
                .context("resource type should have a valid resource string")?;

            let response = shuttle_common::resource::Response {
                r#type,
                config: serde_json::from_slice(&resource.config)
                    .context(format!("{} resource config should be valid JSON", r#type))?,
                data: serde_json::from_slice(&resource.data)
                    .context(format!("{} resource data should be valid JSON", r#type))?,
            };

            Ok(response)
        }
    }

    pub type Client = ResourceRecorderClient<
        shuttle_common::claims::ClaimService<
            shuttle_common::claims::InjectPropagation<tonic::transport::Channel>,
        >,
    >;

    /// Get a resource recorder client that is correctly configured for all services
    pub async fn get_client(resource_recorder_uri: Uri) -> Client {
        let channel = tonic::transport::Endpoint::from(resource_recorder_uri)
            .connect()
            .await
            .expect("failed to connect to resource recorder");

        let resource_recorder_service = tower::ServiceBuilder::new()
            .layer(shuttle_common::claims::ClaimLayer)
            .layer(shuttle_common::claims::InjectPropagationLayer)
            .service(channel);

        ResourceRecorderClient::new(resource_recorder_service)
    }

    #[async_trait]
    impl ResourceDal for Client {
        async fn get_project_resources(
            &mut self,
            project_id: &str,
            token: &str,
        ) -> Result<Vec<shuttle_common::resource::Response>, client::Error> {
            let mut req = tonic::Request::new(ProjectResourcesRequest {
                project_id: project_id.to_string(),
            });

            req.metadata_mut().insert(
                AUTHORIZATION.as_str(),
                format!("Bearer {token}")
                    .parse()
                    .expect("to construct a bearer token"),
            );

            let resp = (*self)
                .get_project_resources(req)
                .await?
                .into_inner()
                .clone();

            let resources = resp
                .resources
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error: anyhow::Error| tonic::Status::internal(error.to_string()))?;

            Ok(resources)
        }
    }

    #[cfg(test)]
    mod tests {
        use async_trait::async_trait;
        use serde_json::json;
        use shuttle_common::{database, resource};
        use test_context::{test_context, AsyncTestContext};
        use tonic::Request;

        use crate::generated::resource_recorder::{record_request, RecordRequest};
        use crate::test_utils::resource_recorder::get_mocked_resource_recorder;

        use super::{get_client, Client, ResourceDal};

        #[async_trait]
        impl AsyncTestContext for Client {
            async fn setup() -> Self {
                let port = get_mocked_resource_recorder().await;

                get_client(format!("http://localhost:{port}").parse().unwrap()).await
            }

            async fn teardown(mut self) {}
        }

        #[test_context(Client)]
        #[tokio::test]
        async fn get_project_resources(mut r_r_client: &mut Client) {
            // First record some resources
            r_r_client
                .record_resources(Request::new(RecordRequest {
                    project_id: "project_1".to_string(),
                    service_id: "service_1".to_string(),
                    resources: vec![
                        record_request::Resource {
                            r#type: "database::shared::postgres".to_string(),
                            config: serde_json::to_vec(&json!({"public": true})).unwrap(),
                            data: serde_json::to_vec(&json!({"username": "test"})).unwrap(),
                        },
                        record_request::Resource {
                            r#type: "database::aws_rds::mariadb".to_string(),
                            config: serde_json::to_vec(&json!({})).unwrap(),
                            data: serde_json::to_vec(&json!({"username": "maria"})).unwrap(),
                        },
                    ],
                }))
                .await
                .unwrap();

            let resources = (&mut r_r_client as &mut dyn ResourceDal)
                .get_project_resources("project_1", "user-1")
                .await
                .unwrap();

            assert_eq!(
                resources,
                vec![
                    resource::Response {
                        r#type: resource::Type::Database(database::Type::Shared(
                            database::SharedEngine::Postgres
                        )),
                        config: json!({"public": true}),
                        data: json!({"username": "test"}),
                    },
                    resource::Response {
                        r#type: resource::Type::Database(database::Type::AwsRds(
                            database::AwsRdsEngine::MariaDB
                        )),
                        config: json!({}),
                        data: json!({"username": "maria"}),
                    }
                ]
            );

            // Getting only RDS resources should filter correctly
            let resources = (&mut r_r_client as &mut dyn ResourceDal)
                .get_project_rds_resources("project_1", "user-1")
                .await
                .unwrap();

            assert_eq!(
                resources,
                vec![resource::Response {
                    r#type: resource::Type::Database(database::Type::AwsRds(
                        database::AwsRdsEngine::MariaDB
                    )),
                    config: json!({}),
                    data: json!({"username": "maria"}),
                }]
            );
        }
    }
}

#[cfg(feature = "builder")]
pub mod builder {
    pub use super::generated::builder::*;
}

#[cfg(feature = "logger")]
pub mod logger {
    use std::str::FromStr;
    use std::time::Duration;

    use chrono::{NaiveDateTime, TimeZone, Utc};
    use prost::bytes::Bytes;
    use tokio::{select, sync::mpsc, time::interval};
    use tonic::{
        async_trait,
        codegen::{Body, StdError},
        Request,
    };
    use tracing::error;

    use shuttle_common::{
        log::{Backend, LogItem as LogItemCommon, LogRecorder},
        DeploymentId,
    };

    pub use super::generated::logger::*;

    impl From<LogItemCommon> for LogItem {
        fn from(value: LogItemCommon) -> Self {
            Self {
                deployment_id: value.id.to_string(),
                log_line: Some(LogLine {
                    tx_timestamp: Some(prost_types::Timestamp {
                        seconds: value.timestamp.timestamp(),
                        nanos: value.timestamp.timestamp_subsec_nanos() as i32,
                    }),
                    service_name: format!("{:?}", value.internal_origin),
                    data: value.line.into_bytes(),
                }),
            }
        }
    }

    impl From<LogItem> for LogItemCommon {
        fn from(value: LogItem) -> Self {
            value
                .log_line
                .expect("log item to have log line")
                .to_log_item_with_id(value.deployment_id.parse().unwrap_or_default())
        }
    }

    impl LogLine {
        pub fn to_log_item_with_id(self, deployment_id: DeploymentId) -> LogItemCommon {
            let LogLine {
                service_name,
                tx_timestamp,
                data,
            } = self;
            let tx_timestamp = tx_timestamp.expect("log to have timestamp");

            LogItemCommon {
                id: deployment_id,
                internal_origin: Backend::from_str(&service_name)
                    .expect("backend name to be valid"),
                timestamp: Utc.from_utc_datetime(
                    &NaiveDateTime::from_timestamp_opt(
                        tx_timestamp.seconds,
                        tx_timestamp.nanos.try_into().unwrap_or_default(),
                    )
                    .unwrap_or_default(),
                ),
                line: String::from_utf8(data).expect("line to be utf-8"),
            }
        }
    }

    impl<I> LogRecorder for Batcher<I>
    where
        I: VecReceiver<Item = LogItem> + Clone + 'static,
    {
        fn record(&self, log: LogItemCommon) {
            self.send(log.into());
        }
    }

    /// Adapter to some client which expects to receive a vector of items
    #[async_trait]
    pub trait VecReceiver: Send {
        type Item;

        async fn receive(&mut self, items: Vec<Self::Item>);
    }

    #[async_trait]
    impl<T> VecReceiver for logger_client::LoggerClient<T>
    where
        T: tonic::client::GrpcService<tonic::body::BoxBody> + Send + Sync + Clone,
        T::Error: Into<StdError>,
        T::ResponseBody: Body<Data = Bytes> + Send + 'static,
        T::Future: Send,
        <T::ResponseBody as Body>::Error: Into<StdError> + Send,
    {
        type Item = LogItem;

        async fn receive(&mut self, items: Vec<Self::Item>) {
            if let Err(error) = self
                .store_logs(Request::new(StoreLogsRequest { logs: items }))
                .await
            {
                error!(
                    error = &error as &dyn std::error::Error,
                    "failed to send batch logs to logger"
                );
            }
        }
    }

    /// Wrapper to batch together items before forwarding them to some vector receiver
    #[derive(Clone)]
    pub struct Batcher<I: VecReceiver> {
        tx: mpsc::UnboundedSender<I::Item>,
    }

    impl<I: VecReceiver + 'static> Batcher<I>
    where
        I::Item: Send,
    {
        /// Create a new batcher around inner with the given batch capacity.
        /// Items will be send when the batch has reached capacity or at the set interval. Whichever comes first.
        pub fn new(inner: I, capacity: usize, interval: Duration) -> Self {
            let (tx, rx) = mpsc::unbounded_channel();

            tokio::spawn(Self::batch(inner, rx, capacity, interval));

            Self { tx }
        }

        /// Create a batcher around inner. It will send a batch of items to inner if a capacity of 256 is reached
        /// or if an interval of 1 second is reached.
        pub fn wrap(inner: I) -> Self {
            Self::new(inner, 256, Duration::from_secs(1))
        }

        /// Send a single item into this batcher
        pub fn send(&self, item: I::Item) {
            if self.tx.send(item).is_err() {
                unreachable!("the receiver will never drop");
            }
        }

        /// Background task to forward the items once the batch capacity has been reached
        async fn batch(
            mut inner: I,
            mut rx: mpsc::UnboundedReceiver<I::Item>,
            capacity: usize,
            interval_duration: Duration,
        ) {
            let mut interval = interval(interval_duration);

            // Without this, the default behaviour will burst any missed tickers until they are caught up.
            // This will cause a flood which we want to avoid.
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

            // Get past the first tick
            interval.tick().await;

            let mut cache = Vec::with_capacity(capacity);

            loop {
                select! {
                    item = rx.recv() => {
                        if let Some(item) = item {
                            cache.push(item);

                            if cache.len() == capacity {
                                let old_cache = cache;
                                cache = Vec::with_capacity(capacity);

                                inner.receive(old_cache).await;
                            }
                        } else {
                            // Sender dropped
                            return;
                        }
                    },
                    _ = interval.tick() => {
                        if !cache.is_empty() {
                            let old_cache = cache;
                            cache = Vec::with_capacity(capacity);

                            inner.receive(old_cache).await;
                        }
                    }
                }
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use std::{
            sync::{Arc, Mutex},
            time::Duration,
        };

        use tokio::time::sleep;
        use tonic::async_trait;

        use super::{Batcher, VecReceiver};

        #[derive(Default, Clone)]
        struct MockGroupReceiver(Arc<Mutex<Option<Vec<u32>>>>);

        #[async_trait]
        impl VecReceiver for MockGroupReceiver {
            type Item = u32;

            async fn receive(&mut self, items: Vec<Self::Item>) {
                *self.0.lock().unwrap() = Some(items);
            }
        }

        #[tokio::test]
        async fn capacity_reached() {
            let mock = MockGroupReceiver::default();
            let batcher = Batcher::new(mock.clone(), 2, Duration::from_secs(120));

            batcher.send(1);
            sleep(Duration::from_millis(50)).await;
            assert_eq!(*mock.0.lock().unwrap(), None);

            batcher.send(2);
            sleep(Duration::from_millis(50)).await;
            assert_eq!(*mock.0.lock().unwrap(), Some(vec![1, 2]));

            batcher.send(3);
            sleep(Duration::from_millis(50)).await;
            assert_eq!(*mock.0.lock().unwrap(), Some(vec![1, 2]));

            batcher.send(4);
            sleep(Duration::from_millis(50)).await;
            assert_eq!(*mock.0.lock().unwrap(), Some(vec![3, 4]));
        }

        #[tokio::test]
        async fn interval_reached() {
            let mock = MockGroupReceiver::default();
            let batcher = Batcher::new(mock.clone(), 2, Duration::from_millis(300));

            sleep(Duration::from_millis(500)).await;
            assert_eq!(
                *mock.0.lock().unwrap(),
                None,
                "we should never send something when the cache is empty"
            );

            batcher.send(1);
            sleep(Duration::from_millis(50)).await;
            assert_eq!(*mock.0.lock().unwrap(), None);

            sleep(Duration::from_millis(500)).await;
            assert_eq!(*mock.0.lock().unwrap(), Some(vec![1]));
        }
    }
}
