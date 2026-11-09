//! Serialize SQLite work on one dedicated thread, away from network tasks.
use crate::store::Store;
use anyhow::{Context, Result};
use std::path::Path;
use tokio::sync::oneshot;

type Work = Box<dyn FnOnce(&mut Store) + Send>;
#[derive(Clone)]
pub struct Db {
    tx: std::sync::mpsc::Sender<Work>,
}
impl Db {
    pub fn open(path: &Path) -> Result<Self> {
        let mut store = Store::open(path)?;
        let (tx, rx) = std::sync::mpsc::channel::<Work>();
        std::thread::Builder::new()
            .name("ticket-database".into())
            .spawn(move || {
                for work in rx {
                    work(&mut store);
                }
            })?;
        Ok(Self { tx })
    }
    pub async fn call<R: Send + 'static>(
        &self,
        f: impl FnOnce(&mut Store) -> Result<R> + Send + 'static,
    ) -> Result<R> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(Box::new(move |store| {
                let _ = tx.send(f(store));
            }))
            .map_err(|_| anyhow::anyhow!("Database worker stopped"))?;
        rx.await
            .context("Database worker did not complete operation")?
    }
}
