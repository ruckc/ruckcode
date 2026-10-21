//! Agent Client Protocol (v1) agent over a bidirectional JSON-RPC 2.0 transport.
//!
//! Wire types come from the official `agent-client-protocol-schema` crate (see [`schema`]); the
//! transport ([`rpc`]) and the dispatch ([`agent`]) are our own.

pub mod agent;
pub mod rpc;
pub mod schema;

pub use agent::{serve, serve_stdio, spawn_in_process, AcpAgent};
pub use rpc::{Connection, Handler, RpcError};
pub use schema::ProtocolVersion;

/// ACP protocol version implemented by this crate.
pub const PROTOCOL_VERSION: ProtocolVersion = ProtocolVersion::V1;
