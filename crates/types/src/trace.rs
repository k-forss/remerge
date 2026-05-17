use serde::{Deserialize, Serialize};

pub const TRACEPARENT_HEADER: &str = "traceparent";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceContext {
    pub trace_id: String,
    pub traceparent: String,
}
