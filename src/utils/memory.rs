//! Return unused allocator pages after bursts of image, JSON and history processing.

/// Bound glibc's per-thread arena retention before the runtime starts its workers.
/// Eight arenas balance concurrent image/JSON processing against idle heap retention.
/// Explicit allocator settings remain authoritative; other allocators are untouched.
pub fn configure() {
  #[cfg(all(target_os = "linux", target_env = "gnu"))]
  {
    if std::env::var_os("MALLOC_ARENA_MAX").is_some()
      || std::env::var_os("GLIBC_TUNABLES").is_some_and(|value| {
        value.to_string_lossy().split(':').any(|part| part.starts_with("glibc.malloc.arena_max="))
      })
    {
      return;
    }
    // SAFETY: mallopt takes integer settings only; this runs before runtime threads start.
    // A failure simply leaves glibc's default policy in place.
    unsafe {
      libc::mallopt(libc::M_ARENA_MAX, 8);
    }
  }
}

/// Best effort only: live allocations and cache contents are never affected.
/// Run infrequently on the blocking pool, away from request and streaming paths.
pub fn release_unused() {
  #[cfg(all(target_os = "linux", target_env = "gnu"))]
  // SAFETY: glibc's malloc_trim is thread-safe, takes no pointers and only releases free pages.
  // Other platforms use their allocator's normal reclamation policy.
  unsafe {
    libc::malloc_trim(0);
  }
}
