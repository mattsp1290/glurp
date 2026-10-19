//! Invocation ownership: a dropped future cancels only its private child.
use tokio_util::sync::{CancellationToken, DropGuard};

pub(crate) struct Invocation {
    pub(crate) token: CancellationToken,
    guard: DropGuard,
}
impl Invocation {
    pub(crate) fn new(parent: &CancellationToken) -> Self {
        let token = parent.child_token();
        let guard = token.clone().drop_guard();
        Self { token, guard }
    }
    pub(crate) fn completed(self) {
        self.guard.disarm();
    }
}
