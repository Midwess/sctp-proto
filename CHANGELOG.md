# Unreleased

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
