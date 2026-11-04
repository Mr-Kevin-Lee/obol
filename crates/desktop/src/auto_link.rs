//! New SimpleFIN accounts join on Refresh: open a new card at a bank
//! that's already connected, and it shows up on the next Refresh with
//! obol's guesses, no relinking.
//!
//! **Contract.** Signatures are final; bodies are agent C's.

use std::sync::Arc;

use obol_core::AccountSet;

use crate::dashboard::StorageLayout;
use crate::simplefin::SimpleFin;

/// The accounts a Refresh is about to see, fetched through the shared
/// provider so the run right after reuses the same request. `None` when no
/// source uses SimpleFIN yet (connecting is the form's job), when there's
/// no Access URL, or when the fetch fails, since the run then reports the
/// failure per source anyway.
pub async fn fetch_for_auto_link(
    _layout: &StorageLayout,
    _simplefin: &SimpleFin,
) -> Option<Arc<AccountSet>> {
    todo!("agent C")
}

/// Links every account in `set` that's neither linked nor ignored, with
/// obol's guess (`simplefin::guess`), and returns one label per account
/// added, `"<institution> <name>"`, for the Refresh banner.
pub fn auto_link_new_accounts(
    _layout: &StorageLayout,
    _set: &AccountSet,
) -> Result<Vec<String>, String> {
    todo!("agent C")
}
