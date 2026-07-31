//! 共用的 JSON-RPC 消息框架（design.md §6.5）。
//!
//! CDP 与 Marionette 两个后端共用：消息编解码、id↔响应匹配、事件路由。
//! V1 实现填充 WebSocket 传输层与匹配器。

use serde::{Deserialize, Serialize};

/// 客户端 → 浏览器 的请求。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcRequest {
    pub id: u64,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

impl RpcRequest {
    pub fn new(id: u64, method: impl Into<String>, params: Option<serde_json::Value>) -> Self {
        Self {
            id,
            method: method.into(),
            params,
        }
    }
}

/// 浏览器 → 客户端 的响应。
#[derive(Debug, Clone, Deserialize)]
pub struct RpcResponse {
    pub id: u64,
    #[serde(default)]
    pub result: Option<serde_json::Value>,
    #[serde(default)]
    pub error: Option<RpcError>,
}

/// 协议层错误。
#[derive(Debug, Clone, Deserialize)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
}

/// 入站消息：命中的响应（含 id）或异步事件（无 id）。
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum Incoming {
    Response(RpcResponse),
    Event {
        method: String,
        #[serde(default)]
        params: Option<serde_json::Value>,
    },
}

/// 下一步 id 分配器（简单递增，单连接使用）。
#[derive(Debug, Default)]
pub struct IdAllocator(u64);

impl IdAllocator {
    pub fn allocate_id(&mut self) -> u64 {
        self.0 += 1;
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_serializes_without_params_when_none() {
        let req = RpcRequest::new(1, "Page.navigate", None);
        let json = serde_json::to_string(&req).unwrap();
        assert!(!json.contains("params"));
        assert!(json.contains("\"id\":1"));
    }

    #[test]
    fn incoming_dispatches_response_vs_event() {
        let resp: Incoming = serde_json::from_str(r#"{"id":3,"result":{"ok":true}}"#).unwrap();
        assert!(matches!(resp, Incoming::Response(r) if r.id == 3));

        let event: Incoming = serde_json::from_str(r#"{"method":"Page.loadEventFired"}"#).unwrap();
        assert!(matches!(event, Incoming::Event { method, .. } if method == "Page.loadEventFired"));
    }

    #[test]
    fn id_allocator_is_monotonic() {
        let mut a = IdAllocator::default();
        assert_eq!(a.allocate_id(), 1);
        assert_eq!(a.allocate_id(), 2);
    }
}
