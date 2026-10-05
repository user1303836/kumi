//! Connections that only read, each lent to one read at a time.

use crate::{StoreError, BUSY_TIMEOUT};
use rusqlite::Connection;
use std::{
    path::Path,
    sync::{Condvar, Mutex},
};

pub(crate) struct Readers {
    free: Mutex<Vec<Connection>>,
    returned: Condvar,
}

impl Readers {
    pub(crate) fn open(path: &Path, count: usize) -> Result<Readers, StoreError> {
        let free = (0..count)
            .map(|_| {
                let connection = Connection::open(path)?;
                connection.busy_timeout(BUSY_TIMEOUT)?;
                connection.execute_batch("PRAGMA query_only = ON; PRAGMA temp_store = MEMORY;")?;
                Ok(connection)
            })
            .collect::<Result<Vec<_>, StoreError>>()?;
        Ok(Readers { free: Mutex::new(free), returned: Condvar::new() })
    }
    pub(crate) fn read<T>(&self, job: impl FnOnce(&Connection) -> Result<T, StoreError>) -> Result<T, StoreError> {
        let lent = Lent { readers: self, connection: Some(self.borrow()) };
        job(lent.connection.as_ref().unwrap())
    }
    fn borrow(&self) -> Connection {
        let mut free = self.free.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        loop {
            if let Some(connection) = free.pop() {
                return connection;
            }
            free = self.returned.wait(free).unwrap_or_else(|poisoned| poisoned.into_inner());
        }
    }
}

/// A lent connection, given back when the read is over, however it ends.
struct Lent<'a> {
    readers: &'a Readers,
    connection: Option<Connection>,
}
impl Drop for Lent<'_> {
    fn drop(&mut self) {
        if let Some(connection) = self.connection.take() {
            self.readers.free.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).push(connection);
            self.readers.returned.notify_one();
        }
    }
}
