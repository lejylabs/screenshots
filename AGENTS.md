# Repository-wide engineering requirements

These requirements apply to the whole application and all future features. Read the relevant sections of `PLAN.md`, especially its performance contract, before changing an execution path.

- Prioritize near-zero idle CPU/wakeups, bounded RAM/GPU memory, and immediate input/presentation. The performance budgets in `PLAN.md` are acceptance targets, not claims that the prototype meets them.
- Use native events and invalidation-driven rendering. Do not add permanent polling timers, hidden redraw loops, eager optional runtimes or background streams for still capture.
- Keep blocking enumeration, capture, image work, filesystem, SQLite and networking off GPUI's foreground executor. Required native event/window operations must remain thread-correct, short and reentrancy-safe.
- Keep frames in memory through selection/crop/preview/clipboard. Do not add PNG/disk round trips, per-copy clipboard verification, full-image undo snapshots or persistent full-desktop previews on cancel.
- Bound queues, caches, decoded image bytes, undo and concurrency. Cancel obsolete jobs; stale callbacks must not affect later sessions or closed windows. Release session pixels/textures when no longer needed.
- Protect user data and privacy: truthful save/copy status, recoverable storage operations, explicit uploads, sanitized exports and no sensitive frame/secret logging by default. Performance never justifies weakening correctness, security, DPI handling or accessibility.
- Choose the smallest complete change. Add abstractions, threads, caches, dependencies or alternate backends only for a demonstrated need; record their resource cost. Do not silently relax budgets or change supported platforms.
- Verify affected behavior. For performance-sensitive changes, use optimized builds and compare appropriate CPU/memory/latency measurements; do not run an exhaustive hardware benchmark for a documentation-only edit. Keep measured evidence separate from targets and skipped checks.
