//! Seeds a Mermoz home with fictional demo mail for UI work and screenshots without
//! touching a real mailbox.
//!
//! MERMOZ_HOME=/some/dir cargo run --release -p mermoz-core --example seed_demo
//! then run mermozd with MERMOZ_NO_SYNC=1 and the same MERMOZ_HOME.

use mermoz_core::paths;
use mermoz_core::store::Store;

fn main() {
    let store = Store::open(&paths::db_path(), &paths::blobs_dir()).expect("open store");
    mermoz_core::demo::seed(&store).expect("seed demo mail");
}
