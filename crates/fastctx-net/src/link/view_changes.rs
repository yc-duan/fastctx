//! This member's view as the hub gives it: whole when asked
//! (`Inner::refresh`), and between asks, what changed in it
//! (`crate::view_delta`), put in the view held when that is the one the
//! change starts from. Either way it is taken in one place (`adopt`). The
//! view kept on disk is written at once after a quiet while, and the changes
//! that follow within `KEEPING` in one write at its end: a member of a
//! network of thousands is told of changes often, and what it reads after
//! starting again the hub corrects as soon as it answers.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use super::{Inner, state};
use crate::directory::View;
use crate::view_delta::ViewDelta;

/// How long after one write of the view the next waits, gathering the
/// changes meanwhile.
const KEEPING: Duration = Duration::from_secs(5);

impl Inner {
    /// The hub told what changed in this member's view: put in the view
    /// held when that is the one it starts from; else the whole view is
    /// asked for again.
    pub(super) fn heard_view(&self, delta: ViewDelta) {
        let current = self.view();
        let applied = (current.revision == delta.base).then(|| crate::view_delta::apply(&current, &delta));
        let error = match applied {
            Some(Ok((mut view, members))) => {
                // What came new from the hub, by this machine's clock.
                for member in
                    view.directory.members.iter_mut().filter(|member| members.contains(&member.name.to_string()))
                {
                    member.since = member.since.map(|at| self.local(at));
                    member.settled = member.settled.map(|at| self.local(at));
                }
                match self.adopt(view) {
                    Ok(()) => {
                        self.node.directory_changed();
                        return;
                    }
                    Err(error) => error,
                }
            }
            Some(Err(error)) => error,
            None => String::new(),
        };
        if !error.is_empty() {
            self.node.log(format!("putting in what changed in the view: {error}; asking for it whole"));
        }
        let Some(inner) = self.this.upgrade() else { return };
        tokio::spawn(async move {
            if let Err(error) = inner.refresh().await {
                inner.node.log(format!("asking the hub for the view: {error}"));
            }
            inner.node.directory_changed();
        });
    }

    /// Takes `view`, its times by this machine's clock, as this member's
    /// view now.
    pub(super) fn adopt(&self, mut view: View) -> Result<(), String> {
        // Who came or went since the hub made it.
        self.with_presences(&mut view);
        if view.hub != self.hub {
            return Err(format!("the hub gave a view of {}, not of {}", view.hub, self.hub));
        }
        let me = self.me();
        if view.me != me {
            // The hub renamed this machine: it says so of this machine's key.
            let renamed = view.me().is_some_and(|member| member.key == self.key.public() && member.named(&me));
            if !renamed {
                return Err(format!("the hub gave the view of {}, not of {me}", view.me));
            }
            self.follow_name(view.me.clone());
        }
        // Its number alone is no change.
        let current = self.view();
        let revision = std::mem::replace(&mut view.revision, current.revision);
        let changed = *current != view;
        view.revision = revision;
        if changed {
            self.keep_view();
            self.follow_addresses(&view);
            let mut current = self.view.lock().expect("never poisoned");
            // Who came or went while it was being taken goes in with it: a
            // change told meanwhile went into the view it replaces.
            self.with_presences(&mut view);
            *current = Arc::from(view);
            drop(current);
            if let Some(this) = self.this.upgrade() {
                this.follow_view();
            }
            self.prune_afoot();
            self.ways_changed();
        } else if revision != current.revision {
            // The same view under the hub's new number, which its next
            // change starts from.
            let mut current = self.view.lock().expect("never poisoned");
            let mut next = View::clone(&current);
            next.revision = revision;
            *current = Arc::new(next);
        }
        self.reroute_if_changed();
        Ok(())
    }

    /// Writes the view held to disk: at once when none was written within
    /// `KEEPING`, else when that has passed, unless a write is due already.
    fn keep_view(&self) {
        if self.keeping.swap(true, Ordering::SeqCst) {
            return;
        }
        let wait = self
            .kept_at
            .lock()
            .expect("never poisoned")
            .map_or(Duration::ZERO, |at| KEEPING.saturating_sub(at.elapsed()));
        let this = self.this.clone();
        tokio::spawn(async move {
            tokio::time::sleep(wait).await;
            let Some(inner) = this.upgrade() else { return };
            inner.keeping.store(false, Ordering::SeqCst);
            *inner.kept_at.lock().expect("never poisoned") = Some(std::time::Instant::now());
            let view = inner.view();
            let path = inner.dir.view();
            match tokio::task::spawn_blocking(move || state::write(&path, &*view)).await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => inner.node.log(format!("keeping the view: {error}")),
                Err(error) => inner.node.log(format!("keeping the view: {error}")),
            }
        });
    }
}
