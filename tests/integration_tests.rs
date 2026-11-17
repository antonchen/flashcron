#[path = "integration_tests/core.rs"]
mod core;
#[cfg(feature = "dep-manager")]
#[path = "integration_tests/packages.rs"]
mod packages;
#[cfg(feature = "web")]
#[path = "integration_tests/web.rs"]
mod web;

#[cfg(feature = "web")]
async fn seed_web_job(db_path: &std::path::Path, name: &str, definition: serde_json::Value) {
    let db = flashcron::db::DatabaseManager::init(db_path.to_str().unwrap())
        .await
        .unwrap();
    let job = serde_json::from_value(definition).unwrap();
    assert!(db.store_job(name.to_owned(), job, false).await.unwrap());
}

#[cfg(feature = "web")]
static IPC_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(feature = "web")]
fn lock_ipc_tests() -> std::sync::MutexGuard<'static, ()> {
    IPC_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
