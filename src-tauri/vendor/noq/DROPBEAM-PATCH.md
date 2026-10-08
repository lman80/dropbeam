# noq 1.3.0 — DropBeam patch

Vendored from crates.io `noq` 1.3.0. Only `src/mutex.rs` changed (search `DropBeam patch`).

The connection `Mutex::lock` tolerates poison (`PoisonError::into_inner`) instead of
`unwrap()`. Before, one panic while a connection was locked (see the noq-proto ACK overflow in
`../noq-proto/DROPBEAM-PATCH.md`) poisoned the mutex; every later lock of that connection
panicked, including `ConnectionRef::drop` during unwinding, and "panic in a destructor during
cleanup" aborted the whole app. With this patch the panicking task (a tokio task, which catches
it) dies alone and the rest of the process keeps serving.

Remove when upstream noq makes connection locking poison-tolerant.
