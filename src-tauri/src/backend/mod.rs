//! tailash 后端（自 gpui 分支整体迁移，不含任何 UI 代码）：
//! worker 线程轮询/操作 mihomo 内核，状态写入 SharedSnapshot 供前端消费。
pub mod bridge;
pub mod config_gen;
pub mod gemini;
pub mod mihomo_api;
pub mod proxy_view;
pub mod runtime;
pub mod subscription;
