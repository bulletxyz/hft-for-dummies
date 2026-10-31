//! Pinning threads to CPU cores.
//!
//! By default the operating system moves threads between cores as it likes.
//! Each move costs a little (cold caches), and a busy neighbour on "your" core
//! makes you wait for a turn. Pinning a hot thread to its own core, ideally one
//! the OS has been told to keep free (`isolcpus`, see Part 8), removes both.

/// Pin the calling thread to `core`, if set. Logs instead of failing: macOS,
/// for one, doesn't let you pin threads.
pub fn pin_current_thread(core: Option<usize>, what: &str) {
    let Some(id) = core else { return };
    if core_affinity::set_for_current(core_affinity::CoreId { id }) {
        tracing::info!("{what} pinned to core {id}");
    } else {
        tracing::warn!("could not pin {what} to core {id} (not supported here?)");
    }
}
