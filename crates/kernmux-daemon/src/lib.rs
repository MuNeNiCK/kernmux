//! Headless Kernmux host-management service.

mod compatibility;
pub mod console;
pub mod host_api;
pub mod image_store;
pub mod inventory;
pub mod lifecycle;
pub mod lifecycle_executor;
pub mod operations;
pub mod placement;
pub mod resource_pool;
pub mod scheduler;
pub mod security;
pub mod service;
pub mod storage_inventory;
pub mod transport;
