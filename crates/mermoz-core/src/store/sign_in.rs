//! Accounts whose Google sign-in was refused: remembered across daemon restarts until the
//! user signs in again, so the account keeps showing "Sign in again" and never retries.

use super::Store;
use crate::error::Result;

fn key(account_id: &str) -> String {
    format!("sign_in_expired:{account_id}")
}

impl Store {
    /// Google refused this account's sign-in and the user has not signed in since.
    pub fn sign_in_expired(&self, account_id: &str) -> Result<bool> {
        Ok(self.setting(&key(account_id))?.is_some())
    }

    pub fn set_sign_in_expired(&self, account_id: &str, expired: bool) -> Result<()> {
        if expired {
            return self.set_setting(&key(account_id), "1");
        }
        self.w
            .lock()
            .unwrap()
            .execute("DELETE FROM settings WHERE key = ?1", [key(account_id)])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::{fixture, rstest};

    struct Env {
        _dir: tempfile::TempDir,
        store: Store,
        acc: String,
    }

    #[fixture]
    fn env() -> Env {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("db.sqlite"), &dir.path().join("blobs")).unwrap();
        let acc = store.add_account("me@example.com", "gmail").unwrap().id;
        Env {
            _dir: dir,
            store,
            acc,
        }
    }

    #[rstest]
    fn a_new_account_is_signed_in(env: Env) {
        assert!(!env.store.sign_in_expired(&env.acc).unwrap());
    }

    #[rstest]
    fn a_refused_sign_in_is_remembered(env: Env) {
        env.store.set_sign_in_expired(&env.acc, true).unwrap();
        assert!(env.store.sign_in_expired(&env.acc).unwrap());
    }

    #[rstest]
    fn signing_in_again_clears_it(env: Env) {
        env.store.set_sign_in_expired(&env.acc, true).unwrap();
        env.store.set_sign_in_expired(&env.acc, false).unwrap();
        assert!(!env.store.sign_in_expired(&env.acc).unwrap());
    }

    #[rstest]
    fn it_survives_reopening_the_store(env: Env) {
        env.store.set_sign_in_expired(&env.acc, true).unwrap();
        let dir = env._dir;
        drop(env.store);
        let store = Store::open(&dir.path().join("db.sqlite"), &dir.path().join("blobs")).unwrap();
        assert!(store.sign_in_expired(&env.acc).unwrap());
    }

    #[rstest]
    fn it_is_per_account(env: Env) {
        let other = env
            .store
            .add_account("other@example.com", "gmail")
            .unwrap()
            .id;
        env.store.set_sign_in_expired(&env.acc, true).unwrap();
        assert!(!env.store.sign_in_expired(&other).unwrap());
    }
}
