//! Agent Client Protocol (v1) wire types and a bidirectional JSON-RPC 2.0 transport.

pub mod rpc;
pub mod schema;

pub use rpc::{Connection, Handler, RpcError};

/// ACP protocol version implemented by this crate.
pub const PROTOCOL_VERSION: u16 = 1;
