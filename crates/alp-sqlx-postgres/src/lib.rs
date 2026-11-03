//! ALP's deliberately PostgreSQL-only SQLx compatibility surface.
//!
//! This crate exists so ALP does not depend on the upstream multi-database
//! `sqlx` facade and therefore does not lock unused MySQL/RSA dependencies.
//! Keep this surface intentionally small and expand only when ALP needs it.

pub use sqlx_core::acquire::Acquire;
pub use sqlx_core::arguments::{Arguments, IntoArguments};
pub use sqlx_core::column::{Column, ColumnIndex};
pub use sqlx_core::connection::{ConnectOptions, Connection};
pub use sqlx_core::database::{self, Database};
pub use sqlx_core::decode::Decode;
pub use sqlx_core::describe::Describe;
pub use sqlx_core::encode::{Encode, IsNull};
pub use sqlx_core::error::{self, Error, Result};
pub use sqlx_core::executor::{Execute, Executor};
pub use sqlx_core::from_row::FromRow;
pub use sqlx_core::pool::{self, Pool};
pub use sqlx_core::query::{query, query_with};
pub use sqlx_core::query_as::{query_as, query_as_with};
pub use sqlx_core::query_builder::{self, QueryBuilder};
pub use sqlx_core::query_scalar::{query_scalar, query_scalar_with};
pub use sqlx_core::raw_sql::{raw_sql, RawSql};
pub use sqlx_core::row::Row;
pub use sqlx_core::statement::Statement;
pub use sqlx_core::transaction::{Transaction, TransactionManager};
pub use sqlx_core::type_info::TypeInfo;
pub use sqlx_core::types::Type;
pub use sqlx_core::value::{Value, ValueRef};
pub use sqlx_core::Either;

pub use sqlx_postgres::{PgConnection, PgExecutor, PgPool, PgPoolOptions, PgTransaction, Postgres};

pub mod postgres {
    pub use sqlx_postgres::*;
}
