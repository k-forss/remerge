# TODO & Future Work

Tracked items, scaffolded code, and planned improvements.
Items are roughly ordered by priority within each section.

---

## Testing

- [ ] Integration tests with a real Docker daemon (behind a feature flag)
- [ ] End-to-end test: CLI → server → worker → binpkg → local emerge
- [ ] Worker container tests with a minimal Gentoo stage3
- [ ] Fuzz testing for portage config parsing and emerge argument filtering
- [ ] Load testing for concurrent workorder submission

---

## Nice-to-have (post v0.1.0)

- [ ] Consider a server-side allowlist of permitted package categories
- [ ] Consider publishing a base worker image to a registry and layering
  the portage config on top at runtime
- [ ] Add OpenTelemetry trace context propagation through the workorder
  lifecycle
- [ ] Queue depth metric (`remerge_queue_depth`) is defined but not updated
  by the queue processor — wire it up
- [ ] `max_workers()` accessor on `DockerManager` is unused — remove or
  wire up
