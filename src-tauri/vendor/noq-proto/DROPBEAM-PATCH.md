# noq-proto 1.3.0 — DropBeam patches

Vendored from crates.io `noq-proto` 1.3.0. Search `DropBeam patch` in `src/`.

1. **BBRv3 Wi-Fi fixes** (opt-in via `Bbr3Config::dropbeam_fixes`) and ACK_FREQUENCY parameters
   handed to every path's controller. See `src/congestion/bbr3/mod.rs`, `src/connection/mod.rs`.

2. **(PATH_)ACK packet overflow → process abort** (upstream n0-computer/noq#367, still open on
   main as of 2026-09-28; no fix after 1.3.0).
   `populate_packet` and the CONNECTION_CLOSE branch write the ACK/PATH_ACK of *every* path into
   one packet with no space check. With several multipath paths whose ACK ranges are fragmented
   (up to 64 ranges each) the frames overflow the packet; encoding through
   `bytes::buf::Limit` then panics (`advance out of bounds: the len is 0 but advancing by 4`)
   while `noq` holds the connection mutex → poisoned mutex → `ConnectionRef::drop` panics while
   unwinding → "panic in a destructor during cleanup" → abort. Seen repeatedly on the Linux
   Transfer Server (many peers, many candidate addresses → many paths).
   Fix:
   - `frame::ack_ranges_that_fit` computes the exact encoded size; `populate_acks` writes only
     the highest ranges that fit (dropping low ranges only acknowledges fewer packets — valid),
     or leaves the ACK pending for the next packet. The close branch reserves room for the
     CONNECTION_CLOSE frame.
   - ACK_FREQUENCY (written unchecked right after the ACKs) now checks its `SIZE_BOUND`.
   - Safety net: `PacketBuilder::write_frame` encodes into the unbounded datagram `Vec` and, if
     a frame exceeds the frame space, truncates it back and logs an error instead of panicking.
   Tests: `frame::test::ack_ranges_that_fit_matches_encoding`,
   `tests::multipath::many_paths_fragmented_acks_do_not_overflow_{packet,close_packet}` (both
   reproduce the exact `bytes` panic without the fix).

Running the crate's own tests (it is excluded from the app workspace): copy `Cargo.toml` to a
scratch dir, append an empty `[workspace]`, symlink `src`, `benches`, `proptest-regressions`,
then `cargo test --lib`.

Remove these patches when upstream ships equivalent fixes.
