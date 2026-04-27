use crate::chunk::{Chunk, chunk_init::ChunkInit, chunk_selective_ack::GapAckBlock};
use crate::config::generate_snap_token;

use super::*;

const ACCEPT_CH_SIZE: usize = 16;

fn create_association(config: TransportConfig) -> Association {
    Association::new(
        None,
        Arc::new(config),
        1400,
        0,
        SocketAddr::from_str("0.0.0.0:0").unwrap(),
        None,
        Instant::now(),
    )
}

fn push_outstanding_chunk(
    a: &mut Association,
    tsn: u32,
    sent_at: Instant,
    payload_len: usize,
) {
    a.inflight_queue.push_no_check(ChunkPayloadData {
        beginning_fragment: true,
        ending_fragment: true,
        tsn,
        since: Some(sent_at),
        nsent: 1,
        user_data: Bytes::from(vec![0; payload_len]),
        ..Default::default()
    });
    a.rack_insert(tsn);
}

fn new_tlr_test_assoc() -> Association {
    Association {
        state: AssociationState::Established,
        mtu: 1200,
        cwnd: 1_000_000,
        rwnd: 1_000_000,
        ssthresh: 1_000_000,
        my_next_tsn: 100,
        my_next_rsn: 100,
        cumulative_tsn_ack_point: 99,
        tlr_burst_first_rtt_units: TLR_BURST_DEFAULT_FIRST_RTT,
        tlr_burst_later_rtt_units: TLR_BURST_DEFAULT_LATER_RTT,
        ..Default::default()
    }
}

fn push_pending_full_packet_chunks(a: &mut Association, n: usize) {
    let user_len = a.mtu as usize - (COMMON_HEADER_SIZE as usize + DATA_CHUNK_HEADER_SIZE as usize);
    assert!(user_len > 0);

    for _ in 0..n {
        a.pending_queue.push(ChunkPayloadData {
            stream_identifier: 0,
            beginning_fragment: true,
            ending_fragment: true,
            user_data: Bytes::from(vec![0; user_len]),
            ..Default::default()
        });
    }
}

fn push_inflight_retransmit_full_packet_chunks(a: &mut Association, start_tsn: u32, n: usize) {
    let user_len = a.mtu as usize - (COMMON_HEADER_SIZE as usize + DATA_CHUNK_HEADER_SIZE as usize);
    assert!(user_len > 0);

    for i in 0..n {
        a.inflight_queue.push_no_check(ChunkPayloadData {
            tsn: start_tsn + i as u32,
            stream_identifier: 0,
            user_data: Bytes::from(vec![0; user_len]),
            nsent: 1,
            retransmit: true,
            ..Default::default()
        });
    }
}

#[test]
fn test_create_forward_tsn_forward_one_abandoned() -> Result<()> {
    let mut a = Association {
        cumulative_tsn_ack_point: 9,
        advanced_peer_tsn_ack_point: 10,
        ..Default::default()
    };

    a.inflight_queue.push_no_check(ChunkPayloadData {
        beginning_fragment: true,
        ending_fragment: true,
        tsn: 10,
        stream_identifier: 1,
        stream_sequence_number: 2,
        user_data: Bytes::from_static(b"ABC"),
        nsent: 1,
        abandoned: true,
        ..Default::default()
    });

    let fwdtsn = a.create_forward_tsn();

    assert_eq!(10, fwdtsn.new_cumulative_tsn, "should be able to serialize");
    assert_eq!(1, fwdtsn.streams.len(), "there should be one stream");
    assert_eq!(1, fwdtsn.streams[0].identifier, "si should be 1");
    assert_eq!(2, fwdtsn.streams[0].sequence, "ssn should be 2");

    Ok(())
}

#[test]
fn test_create_forward_tsn_forward_two_abandoned_with_the_same_si() -> Result<()> {
    let mut a = Association {
        cumulative_tsn_ack_point: 9,
        advanced_peer_tsn_ack_point: 12,
        ..Default::default()
    };

    a.inflight_queue.push_no_check(ChunkPayloadData {
        beginning_fragment: true,
        ending_fragment: true,
        tsn: 10,
        stream_identifier: 1,
        stream_sequence_number: 2,
        user_data: Bytes::from_static(b"ABC"),
        nsent: 1,
        abandoned: true,
        ..Default::default()
    });
    a.inflight_queue.push_no_check(ChunkPayloadData {
        beginning_fragment: true,
        ending_fragment: true,
        tsn: 11,
        stream_identifier: 1,
        stream_sequence_number: 3,
        user_data: Bytes::from_static(b"DEF"),
        nsent: 1,
        abandoned: true,
        ..Default::default()
    });
    a.inflight_queue.push_no_check(ChunkPayloadData {
        beginning_fragment: true,
        ending_fragment: true,
        tsn: 12,
        stream_identifier: 2,
        stream_sequence_number: 1,
        user_data: Bytes::from_static(b"123"),
        nsent: 1,
        abandoned: true,
        ..Default::default()
    });

    let fwdtsn = a.create_forward_tsn();

    assert_eq!(12, fwdtsn.new_cumulative_tsn, "should be able to serialize");
    assert_eq!(2, fwdtsn.streams.len(), "there should be two stream");

    let mut si1ok = false;
    let mut si2ok = false;
    for s in &fwdtsn.streams {
        match s.identifier {
            1 => {
                assert_eq!(3, s.sequence, "ssn should be 3");
                si1ok = true;
            }
            2 => {
                assert_eq!(1, s.sequence, "ssn should be 1");
                si2ok = true;
            }
            _ => panic!("unexpected stream indentifier"),
        }
    }
    assert!(si1ok, "si=1 should be present");
    assert!(si2ok, "si=2 should be present");

    Ok(())
}

#[test]
fn test_handle_forward_tsn_forward_3unreceived_chunks() -> Result<()> {
    let mut a = Association {
        use_forward_tsn: true,
        ..Default::default()
    };

    let prev_tsn = a.peer_last_tsn;

    let fwdtsn = ChunkForwardTsn {
        new_cumulative_tsn: a.peer_last_tsn + 3,
        streams: vec![ChunkForwardTsnStream {
            identifier: 0,
            sequence: 0,
        }],
    };

    let p = a.handle_forward_tsn(&fwdtsn)?;

    let delayed_ack_triggered = a.delayed_ack_triggered;
    let immediate_ack_triggered = a.immediate_ack_triggered;
    assert_eq!(
        a.peer_last_tsn,
        prev_tsn + 3,
        "peerLastTSN should advance by 3 "
    );
    assert!(delayed_ack_triggered, "delayed sack should be triggered");
    assert!(
        !immediate_ack_triggered,
        "immediate sack should NOT be triggered"
    );
    assert!(p.is_empty(), "should return empty");

    Ok(())
}

#[test]
fn test_handle_forward_tsn_forward_1for1_missing() -> Result<()> {
    let mut a = Association {
        use_forward_tsn: true,
        ..Default::default()
    };

    let prev_tsn = a.peer_last_tsn;

    // this chunk is blocked by the missing chunk at tsn=1
    a.payload_queue.push(
        ChunkPayloadData {
            beginning_fragment: true,
            ending_fragment: true,
            tsn: a.peer_last_tsn + 2,
            stream_identifier: 0,
            stream_sequence_number: 1,
            user_data: Bytes::from_static(b"ABC"),
            ..Default::default()
        },
    );

    let fwdtsn = ChunkForwardTsn {
        new_cumulative_tsn: a.peer_last_tsn + 1,
        streams: vec![ChunkForwardTsnStream {
            identifier: 0,
            sequence: 1,
        }],
    };

    let p = a.handle_forward_tsn(&fwdtsn)?;

    let delayed_ack_triggered = a.delayed_ack_triggered;
    let immediate_ack_triggered = a.immediate_ack_triggered;
    assert_eq!(
        a.peer_last_tsn,
        prev_tsn + 2,
        "peerLastTSN should advance by 2"
    );
    assert!(delayed_ack_triggered, "delayed sack should be triggered");
    assert!(
        !immediate_ack_triggered,
        "immediate sack should NOT be triggered"
    );
    assert!(p.is_empty(), "should return empty");

    Ok(())
}

#[test]
fn test_handle_forward_tsn_forward_1for2_missing() -> Result<()> {
    let mut a = Association {
        use_forward_tsn: true,
        ..Default::default()
    };

    a.use_forward_tsn = true;
    let prev_tsn = a.peer_last_tsn;

    // this chunk is blocked by the missing chunk at tsn=1
    a.payload_queue.push(
        ChunkPayloadData {
            beginning_fragment: true,
            ending_fragment: true,
            tsn: a.peer_last_tsn + 3,
            stream_identifier: 0,
            stream_sequence_number: 1,
            user_data: Bytes::from_static(b"ABC"),
            ..Default::default()
        },
    );

    let fwdtsn = ChunkForwardTsn {
        new_cumulative_tsn: a.peer_last_tsn + 1,
        streams: vec![ChunkForwardTsnStream {
            identifier: 0,
            sequence: 1,
        }],
    };

    let p = a.handle_forward_tsn(&fwdtsn)?;

    let immediate_ack_triggered = a.immediate_ack_triggered;
    assert_eq!(
        a.peer_last_tsn,
        prev_tsn + 1,
        "peerLastTSN should advance by 1"
    );
    assert!(
        immediate_ack_triggered,
        "immediate sack should be triggered"
    );
    assert!(p.is_empty(), "should return empty");

    Ok(())
}

#[test]
fn test_handle_forward_tsn_dup_forward_tsn_chunk_should_generate_sack() -> Result<()> {
    let mut a = Association {
        use_forward_tsn: true,
        ..Default::default()
    };

    let prev_tsn = a.peer_last_tsn;

    let fwdtsn = ChunkForwardTsn {
        new_cumulative_tsn: a.peer_last_tsn,
        streams: vec![ChunkForwardTsnStream {
            identifier: 0,
            sequence: 1,
        }],
    };

    let p = a.handle_forward_tsn(&fwdtsn)?;

    let ack_state = a.ack_state;
    assert_eq!(a.peer_last_tsn, prev_tsn, "peerLastTSN should not advance");
    assert_eq!(AckState::Immediate, ack_state, "sack should be requested");
    assert!(p.is_empty(), "should return empty");

    Ok(())
}

#[test]
fn test_assoc_create_new_stream() -> Result<()> {
    let mut a = Association::default();

    for i in 0..ACCEPT_CH_SIZE {
        let stream_identifier =
            if let Some(s) = a.create_stream(i as u16, true, PayloadProtocolIdentifier::Unknown) {
                s.stream_identifier
            } else {
                panic!("{} should success", i);
            };
        let result = a.streams.get(&stream_identifier);
        assert!(result.is_some(), "should be in a.streams map");
    }

    let new_si = ACCEPT_CH_SIZE as u16;
    let result = a.streams.get(&new_si);
    assert!(result.is_none(), "should NOT be in a.streams map");

    let to_be_ignored = ChunkPayloadData {
        beginning_fragment: true,
        ending_fragment: true,
        tsn: a.peer_last_tsn + 1,
        stream_identifier: new_si,
        user_data: Bytes::from_static(b"ABC"),
        ..Default::default()
    };

    let p = a.handle_data(&to_be_ignored)?;
    assert!(p.is_empty(), "should return empty");

    Ok(())
}

#[test]
fn test_handle_sack_marks_rack_loss_for_older_outstanding_chunk() -> Result<()> {
    let now = Instant::now();
    let mut a = Association {
        state: AssociationState::Established,
        mtu: 1200,
        cwnd: 4800,
        rwnd: 4800,
        ssthresh: 4800,
        cumulative_tsn_ack_point: 9,
        rack_highest_delivered_orig_tsn: 9,
        rack_reo_wnd_floor: Duration::ZERO,
        ..Default::default()
    };

    push_outstanding_chunk(&mut a, 10, now - Duration::from_millis(140), 100);
    push_outstanding_chunk(&mut a, 11, now - Duration::from_millis(100), 100);

    let sack = ChunkSelectiveAck {
        cumulative_tsn_ack: 9,
        advertised_receiver_window_credit: 4800,
        gap_ack_blocks: vec![GapAckBlock { start: 2, end: 2 }],
        duplicate_tsn: vec![],
    };

    let packets = a.handle_sack(&sack, now)?;

    assert!(packets.is_empty(), "SACK handling should not emit packets");
    assert!(a.inflight_queue.get(11).is_some_and(|chunk| chunk.acked));
    assert!(a.inflight_queue.get(10).is_some_and(|chunk| chunk.retransmit));
    assert!(a.in_fast_recovery, "RACK loss should enter recovery");
    assert_eq!(1, a.stats.get_num_rack_loss_marks(), "one chunk should be marked lost");
    assert!(a.rack_min_rtt > Duration::ZERO, "RACK min RTT should be updated");

    Ok(())
}

#[test]
fn test_on_rack_timeout_marks_overdue_original_chunk_for_retransmit() {
    let now = Instant::now();
    let mut a = Association {
        state: AssociationState::Established,
        rack_reo_wnd: Duration::from_millis(10),
        rack_delivered_time: Some(now),
        ..Default::default()
    };

    push_outstanding_chunk(&mut a, 10, now - Duration::from_millis(50), 100);

    a.on_rack_timeout(now);

    assert!(
        a.inflight_queue.get(10).is_some_and(|chunk| chunk.retransmit),
        "RACK timeout should mark overdue original transmission lost"
    );
    assert_eq!(1, a.stats.get_num_rack_loss_marks());
}

#[test]
fn test_on_rack_after_sack_duplicate_tsn_inflates_and_decays_reo_wnd() {
    let now = Instant::now();
    let mut a = Association {
        rack_min_rtt: Duration::from_millis(100),
        rack_reo_wnd: Duration::from_millis(25),
        rack_reo_wnd_floor: Duration::ZERO,
        rack_keep_inflated_recoveries: 0,
        ..Default::default()
    };

    let dsack = ChunkSelectiveAck {
        cumulative_tsn_ack: 99,
        advertised_receiver_window_credit: 0,
        gap_ack_blocks: vec![],
        duplicate_tsn: vec![123],
    };

    a.on_rack_after_sack(now, None, 0, false, &dsack);
    assert_eq!(Duration::from_millis(50), a.rack_reo_wnd);
    assert_eq!(15, a.rack_keep_inflated_recoveries);

    let empty = ChunkSelectiveAck {
        cumulative_tsn_ack: 99,
        advertised_receiver_window_credit: 0,
        gap_ack_blocks: vec![],
        duplicate_tsn: vec![],
    };

    a.on_rack_after_sack(now, None, 0, false, &empty);
    assert_eq!(14, a.rack_keep_inflated_recoveries);

    a.rack_keep_inflated_recoveries = 1;
    a.on_rack_after_sack(now, None, 0, false, &empty);
    assert_eq!(0, a.rack_keep_inflated_recoveries);
    assert_eq!(
        Duration::from_millis(25),
        a.rack_reo_wnd,
        "reoWnd should reset to base after duplicate-TSN inflation decays"
    );
}

#[test]
fn test_on_rack_after_sack_suppresses_reo_wnd_during_recovery_without_reordering() {
    let now = Instant::now();
    let mut a = Association {
        rack_reo_wnd: Duration::from_millis(40),
        rack_reordering_seen: false,
        in_fast_recovery: true,
        rack_reo_wnd_floor: Duration::ZERO,
        ..Default::default()
    };

    let empty = ChunkSelectiveAck {
        cumulative_tsn_ack: 0,
        advertised_receiver_window_credit: 0,
        gap_ack_blocks: vec![],
        duplicate_tsn: vec![],
    };

    a.on_rack_after_sack(now, None, 0, false, &empty);
    assert_eq!(Duration::ZERO, a.rack_reo_wnd);

    a.in_fast_recovery = false;
    a.on_rack_after_sack(now, None, 0, false, &empty);
    assert_eq!(
        Duration::ZERO,
        a.rack_reo_wnd,
        "reoWnd should stay suppressed until a valid min-RTT sample exists"
    );

    a.rack_min_rtt_wnd.push(now, Duration::from_millis(120));
    a.on_rack_after_sack(now, None, 0, false, &empty);
    assert_eq!(
        Duration::from_millis(30),
        a.rack_reo_wnd,
        "reoWnd should reinitialize from minRTT/4 after a valid sample"
    );
}

#[test]
fn test_on_rack_after_sack_bounds_reo_wnd_by_srtt() {
    let now = Instant::now();
    let mut a = Association {
        rack_reo_wnd: Duration::from_millis(200),
        rack_reo_wnd_floor: Duration::ZERO,
        ..Default::default()
    };
    a.rto_mgr.set_new_rtt(10);

    let empty = ChunkSelectiveAck {
        cumulative_tsn_ack: 0,
        advertised_receiver_window_credit: 0,
        gap_ack_blocks: vec![],
        duplicate_tsn: vec![],
    };

    a.on_rack_after_sack(now, None, 0, false, &empty);

    assert_eq!(
        Duration::from_millis(10),
        a.rack_reo_wnd,
        "reoWnd must be bounded by SRTT"
    );
}

#[test]
fn test_schedule_pto_uses_configured_single_packet_delayed_ack() {
    let now = Instant::now();
    let mut a = create_association(
        TransportConfig::default().with_rack_worst_case_delayed_ack(Duration::from_millis(75)),
    );
    a.state = AssociationState::Established;
    a.rto_mgr.set_new_rtt(100);

    push_outstanding_chunk(&mut a, 10, now - Duration::from_millis(100), 100);

    a.schedule_pto(now);

    assert_eq!(
        Some(now + Duration::from_millis(275)),
        a.timers.get(Timer::Pto),
        "single-packet PTO should use the configured delayed-ACK allowance"
    );
}

#[test]
fn test_on_rack_after_sack_respects_configured_rack_reo_wnd_floor() {
    let now = Instant::now();
    let mut a = create_association(
        TransportConfig::default().with_rack_reo_wnd_floor(Duration::from_millis(50)),
    );
    a.state = AssociationState::Established;
    a.mtu = 1200;
    a.cwnd = 4800;
    a.rwnd = 4800;
    a.ssthresh = 4800;
    a.cumulative_tsn_ack_point = 9;
    a.rack_highest_delivered_orig_tsn = 9;

    push_outstanding_chunk(&mut a, 10, now - Duration::from_millis(140), 100);
    push_outstanding_chunk(&mut a, 11, now - Duration::from_millis(100), 100);

    let sack = ChunkSelectiveAck {
        cumulative_tsn_ack: 9,
        advertised_receiver_window_credit: 4800,
        gap_ack_blocks: vec![],
        duplicate_tsn: vec![],
    };

    a.on_rack_after_sack(now, Some(now - Duration::from_millis(100)), 11, true, &sack);

    assert_eq!(Duration::from_millis(50), a.rack_reo_wnd);
    assert!(
        a.inflight_queue
            .get(10)
            .is_some_and(|chunk| !chunk.retransmit),
        "configured reoWnd floor should suppress this spurious loss mark"
    );
    assert_eq!(0, a.stats.get_num_rack_loss_marks());
}

#[test]
fn test_on_rack_after_sack_resets_inflated_reo_wnd_to_configured_floor() {
    let now = Instant::now();
    let mut a = create_association(
        TransportConfig::default().with_rack_reo_wnd_floor(Duration::from_millis(50)),
    );
    a.state = AssociationState::Established;
    a.rack_min_rtt = Duration::from_millis(100);
    a.rack_reo_wnd = Duration::from_millis(200);
    a.rack_keep_inflated_recoveries = 1;

    let sack = ChunkSelectiveAck {
        cumulative_tsn_ack: 0,
        advertised_receiver_window_credit: 0,
        gap_ack_blocks: vec![],
        duplicate_tsn: vec![],
    };

    a.on_rack_after_sack(now, None, 0, false, &sack);

    assert_eq!(0, a.rack_keep_inflated_recoveries);
    assert_eq!(
        Duration::from_millis(50),
        a.rack_reo_wnd,
        "reoWnd reset should honor the configured floor"
    );
}

#[test]
fn test_on_pto_timeout_marks_latest_outstanding_chunk_for_probe() -> Result<()> {
    let now = Instant::now();
    let mut a = Association {
        state: AssociationState::Established,
        mtu: 1200,
        cwnd: 4800,
        rwnd: 4800,
        cumulative_tsn_ack_point: 9,
        ..Default::default()
    };

    push_outstanding_chunk(&mut a, 10, now - Duration::from_millis(200), 100);
    push_outstanding_chunk(&mut a, 11, now - Duration::from_millis(100), 100);

    a.on_pto_timeout(now);

    assert!(a.tlr_active, "PTO should begin TLR when inflight data exists");
    assert!(a.tlr_first_rtt, "TLR should start in first-RTT phase");
    assert!(
        a.inflight_queue.get(11).is_some_and(|chunk| chunk.retransmit),
        "latest outstanding TSN should be probed first"
    );
    assert!(
        a.inflight_queue.get(10).is_some_and(|chunk| !chunk.retransmit),
        "older TSNs should not be probed first"
    );
    assert_eq!(1, a.stats.get_num_pto_timeouts(), "PTO counter should increment");
    assert!(a.timers.get(Timer::Pto).is_some(), "PTO timer should be re-armed");

    Ok(())
}

#[test]
fn test_on_pto_timeout_prefers_pending_data_over_probe() -> Result<()> {
    let now = Instant::now();
    let mut a = Association {
        state: AssociationState::Established,
        mtu: 1200,
        cwnd: 4800,
        rwnd: 4800,
        cumulative_tsn_ack_point: 9,
        ..Default::default()
    };

    push_outstanding_chunk(&mut a, 10, now - Duration::from_millis(100), 100);
    a.pending_queue.push(ChunkPayloadData {
        beginning_fragment: true,
        ending_fragment: true,
        stream_identifier: 1,
        user_data: Bytes::from_static(b"pending"),
        ..Default::default()
    });

    a.on_pto_timeout(now);

    assert!(a.tlr_active, "PTO should begin TLR when inflight data exists");
    assert!(
        a.inflight_queue.get(10).is_some_and(|chunk| !chunk.retransmit),
        "pending data should be preferred over a PTO probe"
    );
    assert_eq!(1, a.pending_queue.len(), "pending data should remain queued for normal send");
    assert_eq!(1, a.stats.get_num_pto_timeouts(), "PTO counter should increment");
    assert!(a.timers.get(Timer::Pto).is_some(), "PTO timer should be re-armed");

    Ok(())
}

#[test]
fn test_tlr_allow_send_budget_gating_and_first_send_always_allowed() {
    let now = Instant::now();
    let mut a = new_tlr_test_assoc();
    a.tlr_active = true;
    a.tlr_first_rtt = true;
    a.tlr_start_time = Some(now);

    let mut budget = a.tlr_current_burst_budget_scaled(now);
    let mut consumed = false;
    let mut allowed = 0;
    while a.tlr_allow_send(&mut budget, &mut consumed, a.mtu as usize) {
        allowed += 1;
    }
    assert_eq!(4, allowed, "first RTT TLR budget should allow exactly 4 MTUs");

    let mut budget = 1_000;
    let mut consumed = false;
    let ok1 = a.tlr_allow_send(&mut budget, &mut consumed, a.mtu as usize);
    let ok2 = a.tlr_allow_send(&mut budget, &mut consumed, a.mtu as usize);
    assert!(ok1, "first send should always be allowed");
    assert!(!ok2, "second send should be gated once budget is exhausted");
    assert!(consumed, "budget should be marked consumed");
    assert_eq!(0, budget, "budget should clamp at zero");
}

#[test]
fn test_tlr_begin_sets_end_tsn_to_highest_outstanding() {
    let now = Instant::now();
    let mut a = new_tlr_test_assoc();
    push_inflight_retransmit_full_packet_chunks(&mut a, 100, 5);

    a.tlr_begin(now);

    assert!(a.tlr_active);
    assert!(a.tlr_first_rtt);
    assert!(!a.tlr_had_additional_loss);
    assert_eq!(104, a.tlr_end_tsn);
    assert_eq!(Some(now), a.tlr_start_time);
}

#[test]
fn test_tlr_phase_switches_to_later_on_ack_progress() {
    let now = Instant::now();
    let mut a = new_tlr_test_assoc();
    a.tlr_active = true;
    a.tlr_first_rtt = true;
    a.tlr_start_time = Some(now);
    a.tlr_end_tsn = a.cumulative_tsn_ack_point + 100;

    a.tlr_maybe_finish(now, true);

    assert!(!a.tlr_first_rtt);
    assert_eq!(
        TLR_BURST_DEFAULT_LATER_RTT,
        a.tlr_current_burst_units(now),
        "later RTT burst budget should apply after ACK progress",
    );
}

#[test]
fn test_tlr_first_rtt_expires_by_time_srtt_and_fallback() {
    let now = Instant::now();
    let mut a = new_tlr_test_assoc();

    a.rto_mgr.set_new_rtt(100);
    a.tlr_active = true;
    a.tlr_first_rtt = true;
    a.tlr_start_time = Some(now - Duration::from_millis(150));
    a.tlr_update_phase(now);
    assert!(!a.tlr_first_rtt, "first RTT should expire after SRTT");

    a.rto_mgr.reset();
    a.tlr_active = true;
    a.tlr_first_rtt = true;
    a.tlr_start_time = Some(now - Duration::from_millis(500));
    a.tlr_update_phase(now);
    assert!(a.tlr_first_rtt, "fallback should still be in first RTT before 1s");

    a.tlr_start_time = Some(now - Duration::from_millis(1_100));
    a.tlr_update_phase(now);
    assert!(!a.tlr_first_rtt, "fallback should expire first RTT after 1s");
}

#[test]
fn test_tlr_apply_additional_loss_first_rtt_step_down_and_clamp() {
    let now = Instant::now();
    let mut a = new_tlr_test_assoc();
    a.tlr_active = true;
    a.tlr_first_rtt = true;
    a.tlr_start_time = Some(now);
    a.tlr_had_additional_loss = false;
    a.tlr_good_ops = 7;

    a.tlr_apply_additional_loss(now + Duration::from_millis(10));
    assert!(a.tlr_had_additional_loss);
    assert_eq!(0, a.tlr_good_ops);
    assert_eq!(12, a.tlr_burst_first_rtt_units);
    assert_eq!(TLR_BURST_DEFAULT_LATER_RTT, a.tlr_burst_later_rtt_units);

    a.tlr_apply_additional_loss(now + Duration::from_millis(20));
    a.tlr_apply_additional_loss(now + Duration::from_millis(30));
    assert_eq!(TLR_BURST_MIN_FIRST_RTT, a.tlr_burst_first_rtt_units);
}

#[test]
fn test_tlr_apply_additional_loss_later_rtt_step_down_and_clamp() {
    let now = Instant::now();
    let mut a = new_tlr_test_assoc();
    a.tlr_active = true;
    a.tlr_first_rtt = false;
    a.tlr_start_time = Some(now - Duration::from_secs(10));

    for _ in 0..10 {
        a.tlr_apply_additional_loss(now);
    }

    assert_eq!(TLR_BURST_DEFAULT_FIRST_RTT, a.tlr_burst_first_rtt_units);
    assert_eq!(TLR_BURST_MIN_LATER_RTT, a.tlr_burst_later_rtt_units);
}

#[test]
fn test_tlr_maybe_finish_ends_and_clears_state_and_good_ops_reset() {
    let now = Instant::now();
    let mut a = new_tlr_test_assoc();
    a.tlr_active = true;
    a.tlr_first_rtt = false;
    a.tlr_had_additional_loss = false;
    a.tlr_end_tsn = 200;
    a.cumulative_tsn_ack_point = 200;
    a.tlr_burst_first_rtt_units = TLR_BURST_MIN_FIRST_RTT;
    a.tlr_burst_later_rtt_units = TLR_BURST_MIN_LATER_RTT;
    a.tlr_good_ops = TLR_GOOD_OPS_RESET_THRESHOLD - 1;
    a.tlr_start_time = Some(now);

    a.tlr_maybe_finish(now, false);

    assert!(!a.tlr_active);
    assert!(!a.tlr_first_rtt);
    assert!(!a.tlr_had_additional_loss);
    assert_eq!(0, a.tlr_end_tsn);
    assert_eq!(TLR_BURST_DEFAULT_FIRST_RTT, a.tlr_burst_first_rtt_units);
    assert_eq!(TLR_BURST_DEFAULT_LATER_RTT, a.tlr_burst_later_rtt_units);
    assert_eq!(0, a.tlr_good_ops);
    assert!(a.tlr_start_time.is_none());
}

#[test]
fn test_tlr_pop_pending_data_chunks_to_send_respects_burst_budget_first_rtt() {
    let now = Instant::now();
    let mut a = new_tlr_test_assoc();
    a.tlr_active = true;
    a.tlr_first_rtt = true;
    a.tlr_start_time = Some(now);
    push_pending_full_packet_chunks(&mut a, 10);

    let mut budget = a.tlr_current_burst_budget_scaled(now);
    let mut consumed = false;
    let (chunks, _) = a.pop_pending_data_chunks_to_send(now, &mut budget, &mut consumed);

    assert_eq!(4, chunks.len(), "first RTT budget should allow 4 full-MTU chunks");
    assert_eq!(4, a.inflight_queue.len(), "4 chunks should move to inflight");
    assert_eq!(6, a.pending_queue.len(), "remaining chunks should stay pending");
    assert!(consumed);
    assert_eq!(0, budget);
}

#[test]
fn test_tlr_get_data_packets_to_retransmit_respects_burst_budget_later_rtt() {
    let now = Instant::now();
    let mut a = new_tlr_test_assoc();
    a.tlr_active = true;
    a.tlr_first_rtt = false;
    a.cumulative_tsn_ack_point = 99;
    push_inflight_retransmit_full_packet_chunks(&mut a, 100, 6);

    let mut budget = a.tlr_current_burst_budget_scaled(now);
    let mut consumed = false;
    let packets = a.get_data_packets_to_retransmit(now, &mut budget, &mut consumed);

    let n_chunks: usize = packets.iter().map(|packet| packet.chunks.len()).sum();
    assert_eq!(2, packets.len(), "later RTT budget should allow 2 full-MTU packets");
    assert_eq!(2, n_chunks, "exactly 2 retransmit chunks should be emitted");
    assert!(consumed);
    assert_eq!(0, budget);
}

fn handle_init_test(name: &str, initial_state: AssociationState, expect_err: bool) {
    let mut a = create_association(TransportConfig::default());
    a.set_state(initial_state);
    let pkt = Packet {
        common_header: CommonHeader {
            source_port: 5001,
            destination_port: 5002,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut init = ChunkInit {
        initial_tsn: 1234,
        num_outbound_streams: 1001,
        num_inbound_streams: 1002,
        initiate_tag: 5678,
        advertised_receiver_window_credit: 512 * 1024,
        ..Default::default()
    };
    init.set_supported_extensions();

    let result = a.handle_init(&pkt, &init);
    if expect_err {
        assert!(result.is_err(), "{} should fail", name);
        return;
    } else {
        assert!(result.is_ok(), "{} should be ok", name);
    }
    assert_eq!(
        if init.initial_tsn == 0 {
            u32::MAX
        } else {
            init.initial_tsn - 1
        },
        a.peer_last_tsn,
        "{} should match",
        name
    );
    assert_eq!(1001, a.my_max_num_outbound_streams, "{} should match", name);
    assert_eq!(1002, a.my_max_num_inbound_streams, "{} should match", name);
    assert_eq!(5678, a.peer_verification_tag, "{} should match", name);
    assert_eq!(
        pkt.common_header.source_port, a.destination_port,
        "{} should match",
        name
    );
    assert_eq!(
        pkt.common_header.destination_port, a.source_port,
        "{} should match",
        name
    );
    assert!(a.use_forward_tsn, "{} should be set to true", name);
    assert_eq!(
        512 * 1024,
        a.rwnd,
        "{} rwnd should be initialized from peer's advertised_receiver_window_credit",
        name
    );
    assert_eq!(
        a.rwnd, a.ssthresh,
        "{} ssthresh should be initialized to rwnd",
        name
    );
}

#[test]
fn test_assoc_handle_init() -> Result<()> {
    handle_init_test("normal", AssociationState::Closed, false);

    handle_init_test(
        "unexpected state established",
        AssociationState::Established,
        true,
    );

    handle_init_test(
        "unexpected state shutdownAckSent",
        AssociationState::ShutdownAckSent,
        true,
    );

    handle_init_test(
        "unexpected state shutdownPending",
        AssociationState::ShutdownPending,
        true,
    );

    handle_init_test(
        "unexpected state shutdownReceived",
        AssociationState::ShutdownReceived,
        true,
    );

    handle_init_test(
        "unexpected state shutdownSent",
        AssociationState::ShutdownSent,
        true,
    );

    Ok(())
}

#[test]
fn test_assoc_max_send_message_size_default() -> Result<()> {
    let mut a = create_association(TransportConfig::default());
    assert_eq!(65536, a.max_send_message_size, "should match");

    let ppi = PayloadProtocolIdentifier::Unknown;
    let stream = a.create_stream(1, false, ppi);
    assert!(stream.is_some(), "should succeed");

    if let Some(mut s) = stream {
        let p = Bytes::from(vec![0u8; 65537]);

        if let Err(err) = s.write_sctp(&p.slice(..65536), ppi) {
            assert_ne!(
                Error::ErrOutboundPacketTooLarge,
                err,
                "should be not Error::ErrOutboundPacketTooLarge"
            );
        } else {
            panic!("should be error");
        }

        if let Err(err) = s.write_sctp(&p.slice(..65537), ppi) {
            assert_eq!(
                Error::ErrOutboundPacketTooLarge,
                err,
                "should be Error::ErrOutboundPacketTooLarge"
            );
        } else {
            panic!("should be error");
        }
    }

    Ok(())
}

#[test]
fn test_assoc_max_send_message_size_explicit() -> Result<()> {
    let mut a = create_association(TransportConfig::default().with_max_send_message_size(30000));
    assert_eq!(30000, a.max_send_message_size, "should match");

    let ppi = PayloadProtocolIdentifier::Unknown;
    let stream = a.create_stream(1, false, ppi);
    assert!(stream.is_some(), "should succeed");

    if let Some(mut s) = stream {
        let p = Bytes::from(vec![0u8; 30001]);

        if let Err(err) = s.write_sctp(&p.slice(..30000), ppi) {
            assert_ne!(
                Error::ErrOutboundPacketTooLarge,
                err,
                "should be not Error::ErrOutboundPacketTooLarge"
            );
        } else {
            panic!("should be error");
        }

        if let Err(err) = s.write_sctp(&p.slice(..30001), ppi) {
            assert_eq!(
                Error::ErrOutboundPacketTooLarge,
                err,
                "should be Error::ErrOutboundPacketTooLarge"
            );
        } else {
            panic!("should be error");
        }
    }

    Ok(())
}

#[test]
fn test_assoc_max_receive_message_size_default() -> Result<()> {
    let mut a = create_association(TransportConfig::default());
    assert_eq!(65536, a.max_receive_message_size, "should match");

    let p = Bytes::from(vec![0u8; 65537]);

    let size_ok = ChunkPayloadData {
        beginning_fragment: true,
        ending_fragment: true,
        tsn: a.peer_last_tsn + 1,
        stream_identifier: 1,
        user_data: p.slice(..65536),
        ..Default::default()
    };

    assert!(a.handle_data(&size_ok).is_ok(), "should succeed");

    let too_large = ChunkPayloadData {
        beginning_fragment: true,
        ending_fragment: true,
        tsn: a.peer_last_tsn + 1,
        stream_identifier: 1,
        user_data: p,
        ..Default::default()
    };

    if let Err(err) = a.handle_data(&too_large) {
        assert_eq!(
            Error::ErrInboundPacketTooLarge,
            err,
            "should be Error::ErrInboundPacketTooLarge"
        );
    } else {
        panic!("should be error");
    }

    Ok(())
}

#[test]
fn test_assoc_max_receive_message_size_explicit() -> Result<()> {
    let mut a = create_association(TransportConfig::default().with_max_receive_message_size(1024));
    assert_eq!(1024, a.max_receive_message_size, "should match");

    let first_chunk = ChunkPayloadData {
        beginning_fragment: true,
        ending_fragment: true,
        tsn: a.peer_last_tsn + 1,
        stream_identifier: 1,
        user_data: Bytes::from(vec![0u8; 512]),
        ..Default::default()
    };

    assert!(a.handle_data(&first_chunk).is_ok(), "should succeed");

    let second_chunk = ChunkPayloadData {
        beginning_fragment: true,
        ending_fragment: true,
        tsn: a.peer_last_tsn + 1,
        stream_identifier: 1,
        user_data: Bytes::from(vec![0u8; 513]),
        ..Default::default()
    };

    if let Err(err) = a.handle_data(&second_chunk) {
        assert_eq!(
            Error::ErrInboundPacketTooLarge,
            err,
            "should be Error::ErrInboundPacketTooLarge"
        );
    } else {
        panic!("should be error");
    }

    Ok(())
}

#[test]
fn test_assoc_max_message_size_asymmetric() -> Result<()> {
    let config = TransportConfig::default()
        .with_max_send_message_size(1024)
        .with_max_receive_message_size(30000);

    let mut a = create_association(config);
    assert_eq!(1024, a.max_send_message_size, "should match");
    assert_eq!(30000, a.max_receive_message_size, "should match");

    let ppi = PayloadProtocolIdentifier::Unknown;
    let stream = a.create_stream(1, false, ppi);
    assert!(stream.is_some(), "should succeed");

    if let Some(mut s) = stream {
        let p = Bytes::from(vec![0u8; 1025]);

        if let Err(err) = s.write_sctp(&p.slice(..1024), ppi) {
            assert_ne!(
                Error::ErrOutboundPacketTooLarge,
                err,
                "should be not Error::ErrOutboundPacketTooLarge"
            );
        } else {
            panic!("should be error");
        }

        if let Err(err) = s.write_sctp(&p.slice(..1025), ppi) {
            assert_eq!(
                Error::ErrOutboundPacketTooLarge,
                err,
                "should be Error::ErrOutboundPacketTooLarge"
            );
        } else {
            panic!("should be error");
        }
    }

    let p = Bytes::from(vec![0u8; 30001]);

    let size_ok = ChunkPayloadData {
        beginning_fragment: true,
        ending_fragment: true,
        tsn: a.peer_last_tsn + 1,
        stream_identifier: 1,
        user_data: p.slice(..30000),
        ..Default::default()
    };

    assert!(a.handle_data(&size_ok).is_ok(), "should succeed");

    let too_large = ChunkPayloadData {
        beginning_fragment: true,
        ending_fragment: true,
        tsn: a.peer_last_tsn + 1,
        stream_identifier: 1,
        user_data: p,
        ..Default::default()
    };

    if let Err(err) = a.handle_data(&too_large) {
        assert_eq!(
            Error::ErrInboundPacketTooLarge,
            err,
            "should be Error::ErrInboundPacketTooLarge"
        );
    } else {
        panic!("should be error");
    }

    Ok(())
}

#[test]
fn test_generate_out_of_band_init() {
    let config = TransportConfig::default();
    let init_bytes = generate_snap_token(&config).unwrap();

    // Parse it back to validate
    let parsed = ChunkInit::unmarshal(&init_bytes).unwrap();

    assert!(!parsed.is_ack, "Should be INIT, not INIT ACK");
    assert!(parsed.initiate_tag != 0, "Initiate tag should not be zero");
    // Token always advertises u16::MAX for stream counts;
    // actual limits are applied from TransportConfig during negotiation.
    assert_eq!(
        parsed.num_outbound_streams,
        u16::MAX,
        "Outbound streams should always be u16::MAX in token"
    );
    assert_eq!(
        parsed.num_inbound_streams,
        u16::MAX,
        "Inbound streams should always be u16::MAX in token"
    );
    assert_eq!(
        parsed.advertised_receiver_window_credit,
        config.max_receive_buffer_size(),
        "ARWND should match config"
    );
}

#[test]
fn test_generate_out_of_band_init_with_custom_config() {
    let config = TransportConfig::default()
        .with_max_receive_buffer_size(2_000_000)
        .with_max_num_outbound_streams(256)
        .with_max_num_inbound_streams(512);

    let init_bytes = generate_snap_token(&config).unwrap();
    let parsed = ChunkInit::unmarshal(&init_bytes).unwrap();

    // Token always advertises u16::MAX for stream counts;
    // actual limits are applied from TransportConfig during negotiation.
    assert_eq!(parsed.num_outbound_streams, u16::MAX);
    assert_eq!(parsed.num_inbound_streams, u16::MAX);
    assert_eq!(parsed.advertised_receiver_window_credit, 2_000_000);
}

#[test]
fn test_generate_out_of_band_init_uniqueness() {
    // Each call to generate_snap_token creates a new INIT with unique random tags.
    let config1 = TransportConfig::default();
    let config2 = TransportConfig::default();

    let init1 = generate_snap_token(&config1).unwrap();
    let init2 = generate_snap_token(&config2).unwrap();

    let parsed1 = ChunkInit::unmarshal(&init1).unwrap();
    let parsed2 = ChunkInit::unmarshal(&init2).unwrap();

    // Initiate tags should be different (random)
    assert_ne!(
        parsed1.initiate_tag, parsed2.initiate_tag,
        "Initiate tags should be unique across calls"
    );

    // Initial TSNs should be different (random)
    assert_ne!(
        parsed1.initial_tsn, parsed2.initial_tsn,
        "Initial TSNs should be unique across calls"
    );
}

#[test]
fn test_out_of_band_association_creation() {
    let local_config = Arc::new(TransportConfig::default());
    let remote_config = TransportConfig::default();
    let max_payload_size = 1200;

    let local_init_bytes = generate_snap_token(&local_config).unwrap();
    let remote_init_bytes = generate_snap_token(&remote_config).unwrap();

    let local_init = ChunkInit::unmarshal(&local_init_bytes).unwrap();
    let remote_init = ChunkInit::unmarshal(&remote_init_bytes).unwrap();

    let remote_addr: SocketAddr = "192.168.1.1:5000".parse().unwrap();

    let assoc = Association::new_with_out_of_band_init(
        local_config.clone(),
        max_payload_size,
        remote_addr,
        None,
        local_init.clone(),
        remote_init.clone(),
    )
    .expect("Should create out-of-band init association");

    // Verify the association is in ESTABLISHED state
    assert_eq!(
        assoc.state(),
        AssociationState::Established,
        "Out-of-band init association should be in ESTABLISHED state"
    );

    // Verify handshake is marked complete
    assert!(
        assoc.handshake_completed,
        "Out-of-band init association should have handshake completed"
    );

    // Verify verification tags
    assert_eq!(
        assoc.my_verification_tag, local_init.initiate_tag,
        "My verification tag should match local init"
    );
    assert_eq!(
        assoc.peer_verification_tag, remote_init.initiate_tag,
        "Peer verification tag should match remote init"
    );

    // Verify TSN setup
    assert_eq!(
        assoc.my_next_tsn, local_init.initial_tsn,
        "My next TSN should match local init"
    );
    assert_eq!(
        assoc.peer_last_tsn,
        remote_init.initial_tsn.wrapping_sub(1),
        "Peer last TSN should be remote init TSN - 1"
    );

    // Verify rwnd
    assert_eq!(
        assoc.rwnd, remote_init.advertised_receiver_window_credit,
        "rwnd should match remote advertised credit"
    );
}

#[test]
fn test_out_of_band_association_stream_negotiation() {
    let config = Arc::new(
        TransportConfig::default()
            .with_max_num_outbound_streams(100)
            .with_max_num_inbound_streams(200),
    );

    // Remote uses default config — token always advertises u16::MAX
    let remote_config = TransportConfig::default();

    let local_init_bytes = generate_snap_token(&config).unwrap();
    let remote_init_bytes = generate_snap_token(&remote_config).unwrap();

    let local_init = ChunkInit::unmarshal(&local_init_bytes).unwrap();
    let remote_init = ChunkInit::unmarshal(&remote_init_bytes).unwrap();

    // Token always advertises u16::MAX for stream counts
    assert_eq!(local_init.num_outbound_streams, u16::MAX);
    assert_eq!(local_init.num_inbound_streams, u16::MAX);
    assert_eq!(remote_init.num_outbound_streams, u16::MAX);
    assert_eq!(remote_init.num_inbound_streams, u16::MAX);

    let remote_addr: SocketAddr = "192.168.1.1:5000".parse().unwrap();

    let assoc = Association::new_with_out_of_band_init(
        config.clone(),
        1200,
        remote_addr,
        None,
        local_init,
        remote_init,
    )
    .expect("Should create out-of-band init association");

    // Stream limits should be clamped by the local config, since the
    // remote token always offers u16::MAX.
    // my_max_num_outbound_streams = min(config.max_out=100, remote_in=MAX) = 100
    assert_eq!(
        assoc.my_max_num_outbound_streams, 100,
        "Outbound streams should be clamped by local config"
    );

    // my_max_num_inbound_streams = min(config.max_in=200, remote_out=MAX) = 200
    assert_eq!(
        assoc.my_max_num_inbound_streams, 200,
        "Inbound streams should be clamped by local config"
    );
}

#[test]
fn test_out_of_band_connected_event() {
    let local_config = Arc::new(TransportConfig::default());
    let remote_config = TransportConfig::default();

    let local_init_bytes = generate_snap_token(&local_config).unwrap();
    let remote_init_bytes = generate_snap_token(&remote_config).unwrap();

    let local_init = ChunkInit::unmarshal(&local_init_bytes).unwrap();
    let remote_init = ChunkInit::unmarshal(&remote_init_bytes).unwrap();

    let remote_addr: SocketAddr = "192.168.1.1:5000".parse().unwrap();

    let mut assoc = Association::new_with_out_of_band_init(
        local_config.clone(),
        1200,
        remote_addr,
        None,
        local_init,
        remote_init,
    )
    .expect("Should create out-of-band init association");

    // Poll should return a Connected event
    let event = assoc.poll();
    assert!(
        matches!(event, Some(Event::Connected)),
        "Should emit Connected event, got {:?}",
        event
    );
}

#[test]
fn test_out_of_band_symmetric_setup() {
    // Test that both sides of an out-of-band init association work correctly
    let config_a = Arc::new(TransportConfig::default());
    let config_b = Arc::new(TransportConfig::default());

    let init_a_bytes = generate_snap_token(&config_a).unwrap();
    let init_b_bytes = generate_snap_token(&config_b).unwrap();

    let init_a = ChunkInit::unmarshal(&init_a_bytes).unwrap();
    let init_b = ChunkInit::unmarshal(&init_b_bytes).unwrap();

    let addr_a: SocketAddr = "192.168.1.1:5000".parse().unwrap();
    let addr_b: SocketAddr = "192.168.1.2:5000".parse().unwrap();

    // Create association A (local=A, remote=B)
    let assoc_a = Association::new_with_out_of_band_init(
        config_a.clone(),
        1200,
        addr_b,
        None,
        init_a.clone(),
        init_b.clone(),
    )
    .expect("Should create association A");

    // Create association B (local=B, remote=A)
    let assoc_b = Association::new_with_out_of_band_init(
        config_b.clone(),
        1200,
        addr_a,
        None,
        init_b.clone(),
        init_a.clone(),
    )
    .expect("Should create association B");

    // Verify both are in ESTABLISHED state
    assert_eq!(assoc_a.state(), AssociationState::Established);
    assert_eq!(assoc_b.state(), AssociationState::Established);

    // Verify verification tags are cross-matched
    assert_eq!(assoc_a.my_verification_tag, assoc_b.peer_verification_tag);
    assert_eq!(assoc_b.my_verification_tag, assoc_a.peer_verification_tag);
}

#[test]
fn test_out_of_band_with_forward_tsn_support() {
    let local_config = Arc::new(TransportConfig::default());
    let remote_config = TransportConfig::default();

    let local_init_bytes = generate_snap_token(&local_config).unwrap();
    let remote_init_bytes = generate_snap_token(&remote_config).unwrap();

    let local_init = ChunkInit::unmarshal(&local_init_bytes).unwrap();
    let remote_init = ChunkInit::unmarshal(&remote_init_bytes).unwrap();

    // Verify supported extensions are present
    let mut has_forward_tsn = false;
    for param in &local_init.params {
        if let Some(ext) = param
            .as_any()
            .downcast_ref::<crate::param::param_supported_extensions::ParamSupportedExtensions>(
        ) {
            for ct in &ext.chunk_types {
                if *ct == crate::chunk::chunk_type::CT_FORWARD_TSN {
                    has_forward_tsn = true;
                }
            }
        }
    }
    assert!(
        has_forward_tsn,
        "Generated INIT should include ForwardTSN support"
    );

    let remote_addr: SocketAddr = "192.168.1.1:5000".parse().unwrap();

    let assoc = Association::new_with_out_of_band_init(
        local_config.clone(),
        1200,
        remote_addr,
        None,
        local_init,
        remote_init,
    )
    .expect("Should create out-of-band init association");

    assert!(
        assoc.use_forward_tsn,
        "Out-of-band init association should have ForwardTSN enabled"
    );
}

#[test]
fn test_out_of_band_initial_tsn_zero_wrap() {
    // Test edge case where initial TSN is 0 (wraps to MAX)
    let config = Arc::new(TransportConfig::default());

    let local_init_bytes = generate_snap_token(&config).unwrap();
    let mut remote_init = ChunkInit::unmarshal(&local_init_bytes).unwrap();

    // Set initial TSN to 0 to test the edge case
    remote_init.initial_tsn = 0;
    remote_init.initiate_tag = 12345;

    // Generate a fresh local init for the association
    let actual_local_init_bytes = generate_snap_token(&config).unwrap();
    let local_init = ChunkInit::unmarshal(&actual_local_init_bytes).unwrap();
    let remote_addr: SocketAddr = "192.168.1.1:5000".parse().unwrap();

    let assoc = Association::new_with_out_of_band_init(
        config.clone(),
        1200,
        remote_addr,
        None,
        local_init,
        remote_init,
    )
    .expect("Should create out-of-band init association");

    // peer_last_tsn should be u32::MAX when initial_tsn is 0
    assert_eq!(
        assoc.peer_last_tsn,
        u32::MAX,
        "peer_last_tsn should wrap to MAX when initial_tsn is 0"
    );
}

#[test]
fn test_out_of_band_rwnd_negotiation() {
    let local_config = Arc::new(TransportConfig::default().with_max_receive_buffer_size(500_000));

    let remote_config = TransportConfig::default().with_max_receive_buffer_size(300_000);

    let local_init_bytes = generate_snap_token(&local_config).unwrap();
    let remote_init_bytes = generate_snap_token(&remote_config).unwrap();

    let local_init = ChunkInit::unmarshal(&local_init_bytes).unwrap();
    let remote_init = ChunkInit::unmarshal(&remote_init_bytes).unwrap();

    let remote_addr: SocketAddr = "192.168.1.1:5000".parse().unwrap();

    let assoc = Association::new_with_out_of_band_init(
        local_config.clone(),
        1200,
        remote_addr,
        None,
        local_init,
        remote_init,
    )
    .expect("Should create out-of-band init association");

    // rwnd should be set to remote's advertised receiver window credit
    assert_eq!(
        assoc.rwnd, 300_000,
        "rwnd should be remote's advertised window"
    );
}

#[test]
fn test_enter_loss_recovery_without_reorder_signal_halves_cwnd() {
    let mut a = Association {
        state: AssociationState::Established,
        mtu: 1200,
        cwnd: 100_000,
        ..Default::default()
    };

    a.enter_loss_recovery();

    assert!(a.in_fast_recovery, "RACK loss should enter fast recovery");
    assert_eq!(
        50_000, a.ssthresh,
        "without a reordering signal the response must fall back to standard 50% halving"
    );
    assert_eq!(50_000, a.cwnd, "cwnd should match the new ssthresh");
}

#[test]
fn test_enter_loss_recovery_with_reorder_signal_uses_default_factor() {
    let mut a = Association {
        state: AssociationState::Established,
        mtu: 1200,
        cwnd: 100_000,
        rack_keep_inflated_recoveries: 16,
        ..Default::default()
    };

    a.enter_loss_recovery();

    assert!(a.in_fast_recovery);
    assert_eq!(
        50_000, a.ssthresh,
        "default factor is RFC 4960 halving (50) in both regimes"
    );
    assert_eq!(50_000, a.cwnd);
}

#[test]
fn test_enter_loss_recovery_with_reorder_signal_honors_configured_factor() {
    let mut a = create_association(
        TransportConfig::default().with_rack_recovery_cwnd_factor_percent(80),
    );
    a.state = AssociationState::Established;
    a.mtu = 1200;
    a.cwnd = 100_000;
    a.rack_keep_inflated_recoveries = 16;

    a.enter_loss_recovery();

    assert!(a.in_fast_recovery);
    assert_eq!(
        80_000, a.ssthresh,
        "configured 80% factor should apply when a reordering signal is present"
    );
    assert_eq!(80_000, a.cwnd);
}

#[test]
fn test_enter_loss_recovery_ignores_configured_factor_without_reorder_signal() {
    let mut a = create_association(
        TransportConfig::default().with_rack_recovery_cwnd_factor_percent(80),
    );
    a.state = AssociationState::Established;
    a.mtu = 1200;
    a.cwnd = 100_000;

    a.enter_loss_recovery();

    assert_eq!(
        50_000, a.ssthresh,
        "configured factor must not weaken the response when no reordering signal is present"
    );
    assert_eq!(50_000, a.cwnd);
}

#[test]
fn test_enter_loss_recovery_clamps_to_4_mtu_floor() {
    let mut a = Association {
        state: AssociationState::Established,
        mtu: 1200,
        cwnd: 1000,
        ..Default::default()
    };

    a.enter_loss_recovery();

    assert_eq!(
        4 * 1200,
        a.ssthresh,
        "ssthresh must never drop below 4 * MTU regardless of factor"
    );
}

#[test]
fn test_enter_loss_recovery_is_idempotent_within_window() {
    let mut a = Association {
        state: AssociationState::Established,
        mtu: 1200,
        cwnd: 100_000,
        ..Default::default()
    };

    a.enter_loss_recovery();
    let cwnd_after_first = a.cwnd;

    a.enter_loss_recovery();

    assert_eq!(
        cwnd_after_first, a.cwnd,
        "subsequent calls within the same recovery window must not reduce cwnd further"
    );
}

#[test]
fn test_rack_adaptive_for_relay_profile_has_correct_defaults() {
    let cfg = TransportConfig::for_relay();
    assert!(cfg.get_rack_adaptive());
    assert_eq!(Duration::from_millis(1200), cfg.get_rack_reo_wnd_floor());
}

#[test]
fn test_effective_floor_falls_back_to_static_when_cold() {
    let cfg = TransportConfig::for_relay();
    let a = create_association(cfg);
    assert_eq!(
        Duration::from_millis(1200),
        a.effective_rack_reo_wnd_floor(),
        "cold tracker must use static floor"
    );
}

#[test]
fn test_effective_floor_grows_above_static_with_jitter() {
    let cfg = TransportConfig::for_relay();
    let mut a = create_association(cfg);
    let now = Instant::now();
    for _ in 0..32 {
        a.jitter_tracker.record(now, 1_000_000);
    }
    let eff = a.effective_rack_reo_wnd_floor();
    assert!(
        eff >= Duration::from_micros(1_300_000),
        "effective floor should track p95 × 1.3, got {:?}",
        eff
    );
}

#[test]
fn test_effective_floor_clamped_below_rto_min_minus_100ms() {
    let cfg = TransportConfig::for_relay();
    let mut a = create_association(cfg);
    let now = Instant::now();
    for _ in 0..32 {
        a.jitter_tracker.record(now, 10_000_000);
    }
    let eff = a.effective_rack_reo_wnd_floor();
    let expected_cap = Duration::from_millis(2900);
    assert!(
        eff <= expected_cap,
        "effective floor {:?} must not exceed rto_min - 100ms = {:?}",
        eff,
        expected_cap
    );
}

#[test]
fn test_effective_floor_static_when_adaptive_disabled() {
    let cfg = TransportConfig::for_relay().with_rack_adaptive(false);
    let mut a = create_association(cfg);
    let now = Instant::now();
    for _ in 0..32 {
        a.jitter_tracker.record(now, 5_000_000);
    }
    assert_eq!(
        Duration::from_millis(1200),
        a.effective_rack_reo_wnd_floor(),
        "with rack_adaptive=false the static floor must be returned regardless of samples"
    );
}

#[test]
fn test_jitter_tracker_resets_on_path_change_via_min_rtt_shift() {
    let cfg = TransportConfig::for_relay();
    let mut a = create_association(cfg);
    let now = Instant::now();
    for _ in 0..32 {
        a.jitter_tracker.record(now, 1_000_000);
    }
    a.jitter_tracker
        .maybe_reset_on_path_change(Duration::from_millis(100));
    assert_eq!(32, a.jitter_tracker.len());

    a.jitter_tracker
        .maybe_reset_on_path_change(Duration::from_millis(300));
    assert_eq!(0, a.jitter_tracker.len());
    assert_eq!(
        Duration::from_millis(1200),
        a.effective_rack_reo_wnd_floor(),
        "after path-change reset, effective floor returns to static",
    );
}

#[test]
fn test_jitter_tracker_records_samples_via_process_selective_ack() {
    let cfg = TransportConfig::for_relay();
    let mut a = create_association(cfg);
    a.state = AssociationState::Established;
    a.cumulative_tsn_ack_point = 99;

    let t0 = Instant::now();
    push_outstanding_chunk(&mut a, 100, t0, 100);
    push_outstanding_chunk(&mut a, 101, t0 + Duration::from_millis(50), 100);
    push_outstanding_chunk(&mut a, 102, t0 + Duration::from_millis(100), 100);

    let sack = ChunkSelectiveAck {
        cumulative_tsn_ack: 102,
        advertised_receiver_window_credit: 1_000_000,
        gap_ack_blocks: vec![],
        duplicate_tsn: vec![],
    };

    let now = t0 + Duration::from_millis(900);
    a.process_selective_ack(&sack, now)
        .expect("ack processing should succeed");

    assert!(
        a.jitter_tracker.len() >= 2,
        "should record samples for nsent==1 chunks; got {}",
        a.jitter_tracker.len()
    );
}

#[test]
fn test_jitter_tracker_skips_retransmitted_chunks() {
    let cfg = TransportConfig::for_relay();
    let mut a = create_association(cfg);
    a.state = AssociationState::Established;
    a.cumulative_tsn_ack_point = 99;

    let t0 = Instant::now();
    let user_len = 100;
    a.inflight_queue.push_no_check(ChunkPayloadData {
        beginning_fragment: true,
        ending_fragment: true,
        tsn: 100,
        since: Some(t0),
        nsent: 2,
        user_data: Bytes::from(vec![0; user_len]),
        ..Default::default()
    });
    a.rack_insert(100);
    a.inflight_queue.push_no_check(ChunkPayloadData {
        beginning_fragment: true,
        ending_fragment: true,
        tsn: 101,
        since: Some(t0 + Duration::from_millis(50)),
        nsent: 1,
        user_data: Bytes::from(vec![0; user_len]),
        ..Default::default()
    });
    a.rack_insert(101);

    let sack = ChunkSelectiveAck {
        cumulative_tsn_ack: 101,
        advertised_receiver_window_credit: 1_000_000,
        gap_ack_blocks: vec![],
        duplicate_tsn: vec![],
    };
    let now = t0 + Duration::from_millis(900);
    a.process_selective_ack(&sack, now)
        .expect("ack processing should succeed");

    assert_eq!(
        1,
        a.jitter_tracker.len(),
        "only the nsent==1 chunk should contribute a sample"
    );
}

#[test]
fn test_mark_rack_losses_records_delta_into_jitter_tracker() {
    let cfg = TransportConfig::for_relay();
    let mut a = create_association(cfg);
    a.state = AssociationState::Established;
    a.cumulative_tsn_ack_point = 99;
    a.rack_reo_wnd = Duration::from_millis(100);

    let t0 = Instant::now();
    push_outstanding_chunk(&mut a, 100, t0, 100);
    push_outstanding_chunk(&mut a, 101, t0 + Duration::from_millis(10), 100);

    let delivered = t0 + Duration::from_millis(800);
    let now = delivered;
    let marked = a.mark_rack_losses(now, delivered);
    assert!(marked, "chunks past reo_wnd must be marked");
    assert_eq!(
        2,
        a.jitter_tracker.len(),
        "each marked chunk must inject a delta sample so the tracker can see the tail"
    );
}

#[test]
fn test_mark_rack_losses_does_not_record_when_adaptive_disabled() {
    let cfg = TransportConfig::for_relay().with_rack_adaptive(false);
    let mut a = create_association(cfg);
    a.state = AssociationState::Established;
    a.cumulative_tsn_ack_point = 99;
    a.rack_reo_wnd = Duration::from_millis(100);

    let t0 = Instant::now();
    push_outstanding_chunk(&mut a, 100, t0, 100);

    let delivered = t0 + Duration::from_millis(800);
    a.mark_rack_losses(delivered, delivered);
    assert_eq!(
        0,
        a.jitter_tracker.len(),
        "with rack_adaptive=false the tracker stays empty"
    );
}

#[test]
fn test_effective_floor_holds_at_historical_after_window_decay() {
    let cfg = TransportConfig::for_relay();
    let mut a = create_association(cfg);
    let now = Instant::now();

    // Simulate a hot phase: 32 samples at 1.5s -- this is the path's
    // observed envelope, slightly above static (1200ms). Record the peak.
    for _ in 0..32 {
        a.jitter_tracker.record(now, 1_500_000);
    }
    if let Some(p95) = a.jitter_tracker.p95() {
        a.jitter_tracker.note_p95(p95);
    }
    assert!(a.jitter_tracker.historical_max_p95().unwrap() >= 1_500_000);

    // Simulate decay: window evicts the hot samples (record one calm sample
    // far in the future and let the tracker drop the rest by time).
    a.jitter_tracker.record(now + Duration::from_secs(60), 100);
    assert_eq!(1, a.jitter_tracker.len());

    // Even with a tiny live p95, the effective floor must hold at the
    // observed historical peak (1.5s), not fall back to static (1.2s).
    // The next jitter burst lands in an already-warmed window.
    let eff = a.effective_rack_reo_wnd_floor();
    assert_eq!(
        Duration::from_micros(1_500_000),
        eff,
        "effective floor must stay at historical_max (1.5s) after window decay; got {:?}",
        eff
    );

    // Historical-max contribution is bounded at 2× static_floor to prevent
    // the bufferbloat-driven runaway ratchet observed in production at 05:47.
    // An arbitrarily large historical peak gets clamped at 2 × 1200ms = 2400ms.
    a.jitter_tracker.note_p95(8_000_000);
    let eff2 = a.effective_rack_reo_wnd_floor();
    assert_eq!(
        Duration::from_micros(2_400_000),
        eff2,
        "historical contribution must be bounded at 2× static_floor; got {:?}",
        eff2
    );
}

#[test]
fn test_apply_transport_config_runtime_swaps_knobs_and_resets_tracker() {
    let mut a = create_association(TransportConfig::default());
    let now = Instant::now();
    for _ in 0..32 {
        a.jitter_tracker.record(now, 1_000_000);
    }
    assert_eq!(32, a.jitter_tracker.len());

    let relay = TransportConfig::for_relay();
    a.apply_transport_config_runtime(&relay);

    assert_eq!(Duration::from_millis(1200), a.rack_reo_wnd_floor);
    assert_eq!(70, a.rack_recovery_cwnd_factor_percent);
    assert_eq!(3000, a.rto_mgr.rto_min);
    assert!(a.rack_adaptive);
    assert_eq!(
        0,
        a.jitter_tracker.len(),
        "tracker must reset on path swap"
    );

    let direct = TransportConfig::default();
    a.apply_transport_config_runtime(&direct);
    assert_eq!(DEFAULT_RACK_REO_WND_FLOOR, a.rack_reo_wnd_floor);
    assert_eq!(
        DEFAULT_RACK_RECOVERY_CWND_FACTOR_PERCENT,
        a.rack_recovery_cwnd_factor_percent
    );
    assert_eq!(1000, a.rto_mgr.rto_min);
}
