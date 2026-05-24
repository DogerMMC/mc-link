use std::sync::{Arc, Mutex};
use std::sync::atomic::AtomicBool;

pub struct AppState {
    pub current_room: Arc<Mutex<Option<(String, String)>>>,
    pub is_running: Arc<Mutex<bool>>,
    pub latency_ms: Arc<Mutex<u64>>,
    pub stop_signal: Arc<Mutex<Option<Arc<AtomicBool>>>>,
}