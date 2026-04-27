# Unreleased

# 0.10.2

  * Restore `for_relay()` static `rack_reo_wnd_floor` from 400ms back to **1200ms** (the 0.9.5 baseline). The 0.10.0 thesis -- "lower static, let adaptive lift" -- only works when RACK marks are a meaningful fraction of the sample window. Production trace at `2026-04-27T05:13` showed a relay path with a tight 400-450ms envelope and only ~5% mark-density. On that path the sampling fix lands the marks in the tracker correctly, but they're too few to drag p95 above the in-order bulk, so the dynamic floor stays at the static value -- and 400ms was below the path's natural envelope, causing chronic RACK over-firing and cwnd grind. The lesson: static floor must be set conservatively against observed historical worst case; adaptive lifts above it only on outlier paths where marks dominate the window. Slot `b534cfb3` in the same trace, with envelope 400-1210ms and dense marks, confirmed the adaptive lift works (p95 climbed `400k -> 893k`, `reo_wnd_us` reached 1163ms). With static back at 1200ms, light-mark slots regain the 0.9.5 throughput baseline; heavy-mark slots still get adaptive lift on top.
  * No algorithm changes -- 0.10.1's adaptive sampling and `apply_transport_config_runtime` are unchanged.
  * 209/209 tests green.

# 0.10.1

  * **Adaptive RACK sampling fix.** Previously the `JitterTracker` recorded `delta_us` only for `nsent == 1` chunks at SACK time (Karn's filter). But the chunks that actually experience large jitter are exactly the ones RACK marks and retransmits — once retransmitted, they carry `nsent ≥ 2` and were silently excluded from the very distribution they should dominate. Production trace at `2026-04-26T18:39` showed `reo_jit_p95_us` of 200-9000 µs while RACK marks were firing at delta=400-700 ms: the estimator was constitutionally blind to its own tail. Now `mark_rack_losses` records each marked chunk's `delta_us` directly into the tracker, so p95 reflects the actual reorder envelope and `effective_rack_reo_wnd_floor` lifts above the static floor when needed.
  * `Association::apply_transport_config_runtime(&TransportConfig)` — live-swap the runtime-tunable RACK / RTO knobs (`rack_reo_wnd_floor`, `rack_recovery_cwnd_factor_percent`, `rack_adaptive`, `rack_worst_case_delayed_ack`, `max_cwnd_bytes`, `rto_min_ms`, `rto_max_ms`) on an established association. Resets the `JitterTracker` so stale samples from the prior path don't bias the new one. Intended for upper layers (e.g. ICE) that want to swap presets when the active path type changes (host/srflx ↔ relay) without tearing down the association.
  * 209/209 tests green (206 prior + 2 mark-time-sampling + 1 runtime-config-swap).

# 0.10.0

  * Adaptive RACK reordering window. The static `rack_reo_wnd_floor` knob, which we tuned six times in one day chasing a moving target (250ms → 400ms → 500ms → 800ms → 1200ms → and slot-dependent), is now a *minimum* below which the dynamic value cannot fall. A per-association `JitterTracker` records `delivered_send_time − chunk.send_time` for `nsent == 1` chunks, computes a sliding-window p95 (256-sample / 30-second cap), and scales by 1.3 to set the effective floor. Clamped above the static floor and below `rto_min_ms − 100ms` so RACK still fires before T3-rtx.
  * `TransportConfig` gains `with_rack_adaptive(bool)` (default **true**). Set to `false` to restore exact pre-0.10 behavior. The static `rack_reo_wnd_floor` setter still works as before; it now serves as the cold-start / minimum value.
  * `for_relay()` lowers its static floor from 1200ms to **400ms** — matching the lower-jitter cluster of TURN paths. The adaptive estimator handles the upper tail dynamically. Operators who want the prior conservative value can call `.with_rack_reo_wnd_floor(Duration::from_millis(1200))` after `for_relay()`.
  * `AssociationSnapshot` gains `rack_jitter_p95_us: u64` and `rack_jitter_sample_count: u32` fields so the algorithm is observable in production. `0` for `rack_jitter_p95_us` indicates either cold start (< 16 samples) or `rack_adaptive=false`.
  * `sctp-stats` log line includes `reo_jit_p95_us=` and `reo_jit_n=` fields.
  * Path-change detection: when `rack_min_rtt` shifts by > 50% in either direction, the tracker resets so old samples don't mislead the new path.
  * Production observation that motivated this work (`2026-04-26T17:47`): static `reo_wnd_floor=1200ms` was correct for two slots but the third slot's path had a wider envelope (~1290ms) and that slot RACK-thrashed at ~5 Mbps while the others ran at 25 Mbps. Adaptive lets each association find its own envelope without per-region retuning.
  * Bumps version 0.9.5 → 0.10.0 (minor: behavior-change-by-default).
  * 206/206 tests green (185 prior + 12 jitter_tracker unit + 8 SACK-driven integration + 1 config).

# 0.9.5

  * Raise `for_relay()` `rack_reo_wnd_floor` from 800ms to 1200ms. Production trace at `2026-04-26T17:47` showed the path's jitter envelope under deeper bursts extends well past 800ms — RACK marks fired at `delta_us` between 800,000 and **1,051,425** µs across all three slots, halving cwnd repeatedly despite the chunks not being lost (just delayed by the larger relay queue depth that grows with cwnd). All three slots dropped from 880KB / 540KB / 87KB cwnd down to 21KB-40KB through ~11+ RACK events each. With `rto_min_ms=3000` already shipped in 0.9.4, there's headroom for `reo_wnd_floor=1200ms` without any T3-vs-RACK race; T3 still fires at 3s as the safety net for genuinely lost packets RACK fails to catch. Real-loss detection latency increases by 400ms in the worst case, which is acceptable for bulk relay transfers.

# 0.9.4

  * `TransportConfig::for_relay()` now also sets `rto_min_ms=3000` so RACK has a clear 2-second window between its 800 ms reorder floor and the T3-rtx safety net. Production trace at `2026-04-26T17:29` showed `T3-rtx timed out: n_rtos=1 cwnd=1228` events on a slot whose jitter envelope expanded to 800-1000+ ms briefly under load — RTO at 1 s was racing RACK at 800 ms and winning, slamming cwnd to 1 MTU. With `rto_min_ms=3000` the race goes away: RACK at 800 ms always fires first, and only genuinely lost-and-RACK-missed packets wait for the slower RTO. The cost is +2 s recovery latency on rare RACK-misses, which is acceptable for bulk relay transfers.

# 0.9.3

  * Retune `TransportConfig::for_relay()` based on production data:
    - Raise `rack_reo_wnd_floor` from 500ms to 800ms. Production logs at `2026-04-26T17:14` showed every RACK mark sitting at `delta_us = 500-700ms`, exactly on the prior floor — the floor was inside the path's natural delivery envelope, so RACK was firing on jitter, not loss. 800ms sits comfortably above the observed envelope while still well below the 1s T3-rtx safety net.
    - Drop the `max_cwnd_bytes=Some(500_000)` cap (now `None`). The cap created a resonant failure mode: cwnd parked at the BDP-equivalent value, the relay queue stayed exactly full, any 50-100ms wobble pushed deliveries past `reo_wnd_floor`, mass RACK marks triggered, T3-rtx cascaded. Logs at `2026-04-26T17:14:40.903` show the collapse: cwnd hit 500_000, then 200ms later T3 fired with cwnd=1228. App-level reliable-channel watermarks already provide burst control upstream of SCTP; the SCTP-level cap was redundant and harmful.
  * `for_relay()` now applies `rack_reo_wnd_floor=800ms`, `rack_recovery_cwnd_factor_percent=70` (gated; only fires on observed reordering), `max_init_retransmits=None`, `max_data_retransmits=None`, and the crate default for `max_cwnd_bytes` (None).

# 0.9.2

  * Add `TransportConfig::for_relay()` profile bundling `rack_reo_wnd_floor=500ms`, `max_cwnd_bytes=Some(500_000)`, `rack_recovery_cwnd_factor_percent=70`, `max_init_retransmits=None`, `max_data_retransmits=None` for TURN-relayed paths.
  * Promote the periodic `sctp-stats` log line from `debug!` to `info!` when the association is stuck (in fast recovery, T3-rtx fired since last log, or pending bytes > 4 × cwnd) so production diagnoses do not require enabling debug logs.

# 0.9.1

  * Default `rack_recovery_cwnd_factor_percent` to 50 (RFC 4960 halving).
  * Gate RACK soft cwnd factor on observed reordering signal (`rack_keep_inflated_recoveries > 0`); otherwise fall back to standard halving.

# 0.9.0

  * Add support for out of band negotiation for SNAP #34
  * Deduplicate incoming RE-CONFIG requests to prevent new stream destruction #30
  * Replace lazy_static with std::sync::LazyLock in tests #41
  * Preparation for no_std + alloc #36

# 0.8.1

  * Downgrade rand to 0.9 to avoid double chacha20 dep #39

# 0.8.0 (yanked)
  
  * Add I-FORWARD-TSN (RFC 8260) chunk support #29
  * MSRV 1.85, Edition 2024, bump deps #38
  * Enforce receive-side max message size #27

# 0.7.1

  * Fix server-side rwnd initialization from INIT #28 

# 0.7.0

  * Mark some types non_exhaustive (breaking) #26
  * Sync sctp-proto with rtc-sctp (breaking) #25
  * Changes to adopt sctp-proto #24

# 0.6.0

  * Configurable SCTP retransmission limits and RTO values #22

# 0.5.0

  * Switch to maintained version of rustc-hash #21

# 0.4.0

  * Update rand crate to 0.9.1 #19
  * Clippy fixes #18
  * Update thiserror to 2.0.16 #17
  * Clippy fixes #16

# 0.3.0

  * Ignore unknown parameters (breaking) #14
  * Port CRC optimizations from webrtc-rs/sctp made by #13

# 0.2.2

  * Move per packet log from debug to trace #12

# 0.2.1

  * Don't log user initiated abort as err #11

# 0.2.0

  * Wrap around ssn to 0 and avoid panic #9
  * Clippy and rust analyzer warnings #10

# 0.1.7

  * Fix T3RTX timer starvation #7

# 0.1.6

  * Respond with ParamOutgoingResetRequest #6

# 0.1.5

  * Fix sequence_number.wrapping_add

# 0.1.4

  * Remove unused deps and update deps

# 0.1.3

  * Make API Sync (as well as Send) and write_with_ppi() #4
  * Chores (clippy and deps) #3
  * Configurable max_payload_size #2
  * Fix build #1
