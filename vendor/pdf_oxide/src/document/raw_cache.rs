//! Modified by fastctx: release stream buffers between read-only pages.

use super::{BoundedObjectCache, Object};

impl BoundedObjectCache {
    pub(super) fn release_streams(&mut self) {
        let mut removed_bytes = 0;
        self.map.retain(|key, object| {
            // Recovered compressed objects are not streams and may have no
            // usable xref entry. Keep them, along with pinned synthetic objects.
            let keep = self.pinned.contains(key) || !matches!(object, Object::Stream { .. });
            if !keep {
                removed_bytes += Self::estimate_size(object);
            }
            keep
        });
        self.current_bytes -= removed_bytes;
        self.insertion_order.retain(|key| self.map.contains_key(key));
    }
}
