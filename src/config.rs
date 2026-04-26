use crate::util::{AssociationIdGenerator, RandomAssociationIdGenerator};

use alloc::boxed::Box;
use alloc::sync::Arc;
use bytes::Bytes;
use core::fmt;
use core::time::Duration;
use thiserror::Error;

/// MTU for inbound packet (from DTLS)
pub(crate) const RECEIVE_MTU: usize = 8192;
/// initial MTU for outgoing packets (to DTLS)
pub(crate) const INITIAL_MTU: u32 = 1228;
pub(crate) const INITIAL_RECV_BUF_SIZE: u32 = 1024 * 1024;
pub(crate) const COMMON_HEADER_SIZE: u32 = 12;
pub(crate) const DATA_CHUNK_HEADER_SIZE: u32 = 16;
pub(crate) const DEFAULT_MAX_MESSAGE_SIZE: u32 = 65536;

// Default RTO values in milliseconds (RFC 4960)
pub(crate) const RTO_INITIAL: u64 = 3000;
pub(crate) const RTO_MIN: u64 = 1000;
pub(crate) const RTO_MAX: u64 = 60000;

// Default max retransmit value (RFC 4960 Section 15)
const DEFAULT_MAX_INIT_RETRANS: usize = 8;

pub(crate) const DEFAULT_RACK_MIN_RTT_WINDOW: Duration = Duration::from_secs(30);
pub(crate) const DEFAULT_RACK_REO_WND_FLOOR: Duration = Duration::from_millis(250);
pub(crate) const DEFAULT_RACK_WORST_CASE_DELAYED_ACK: Duration = Duration::from_millis(200);
pub(crate) const DEFAULT_RACK_RECOVERY_CWND_FACTOR_PERCENT: u8 = 50;

/// Minimum cwnd cap allowed when `max_cwnd_bytes` is `Some(_)`. Equal to the
/// RFC 4960 §7.2.1 initial cwnd lower bound, so that a configured cap can never
/// freeze the association before it has sent its first window.
pub(crate) const MIN_MAX_CWND_BYTES: u32 = 4380;

/// Errors returned when a [`TransportConfig`] contains invalid values.
#[non_exhaustive]
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum TransportConfigError {
    /// RACK minimum RTT window must be strictly positive.
    #[error("invalid RACK minimum RTT window: must be greater than zero")]
    InvalidRackMinRttWindow,
    /// RACK worst-case delayed ACK allowance must be strictly positive.
    #[error("invalid RACK worst-case delayed ACK: must be greater than zero")]
    InvalidRackWorstCaseDelayedAck,
    /// RACK recovery cwnd factor must be in the range 1..=100.
    #[error("invalid RACK recovery cwnd factor: must be in the range 1..=100")]
    InvalidRackRecoveryCwndFactor,
    /// A configured cwnd cap must be at least `MIN_MAX_CWND_BYTES`.
    #[error("invalid max cwnd bytes: Some(v) requires v >= 4380")]
    InvalidMaxCwndBytes,
}

/// Config collects the arguments to create_association construction into
/// a single structure
#[derive(Debug, Clone)]
pub struct TransportConfig {
    max_receive_buffer_size: u32,
    max_num_outbound_streams: u16,
    max_num_inbound_streams: u16,

    /// Maximum message size we will SEND (respects remote's advertised limit)
    /// Can be updated after association creation via set_max_send_message_size()
    max_send_message_size: u32,

    /// Maximum message size we will RECEIVE (what we advertise in SDP)
    /// Enforced during reassembly - messages exceeding this are rejected
    max_receive_message_size: u32,

    /// Maximum number of retransmissions for INIT chunks during handshake.
    /// Set to `None` for unlimited retries (recommended for WebRTC).
    /// Default: Some(8)
    max_init_retransmits: Option<usize>,

    /// Maximum number of retransmissions for DATA chunks.
    /// Set to `None` for unlimited retries (recommended for WebRTC).
    /// Default: None (unlimited)
    max_data_retransmits: Option<usize>,

    /// Initial retransmission timeout in milliseconds.
    /// Default: 3000
    rto_initial_ms: u64,

    /// Minimum retransmission timeout in milliseconds.
    /// Default: 1000
    rto_min_ms: u64,

    /// Maximum retransmission timeout in milliseconds.
    /// Default: 60000
    rto_max_ms: u64,

    /// Window length used to track the minimum RTT for RACK.
    /// Default: 30 seconds.
    rack_min_rtt_window: Duration,

    /// Floor applied to the derived RACK reordering window.
    /// Default: 0.
    rack_reo_wnd_floor: Duration,

    /// Worst-case delayed-ACK allowance used for PTO when only one packet is in flight.
    /// Default: 200 milliseconds.
    rack_worst_case_delayed_ack: Duration,

    /// Multiplier (as a percentage in 1..=100) applied to cwnd on RACK-triggered fast
    /// recovery, but only when recent SACKs have proven reordering is occurring (the
    /// peer reported `duplicate_tsn` entries within the last 16 SACKs, i.e. the same
    /// signal that inflates `rack_reo_wnd`). When no reordering signal is present the
    /// reduction falls back to the standard RFC 4960 halving (50%), so this knob never
    /// weakens the response to genuinely congestive loss.
    ///
    /// Lower values are more aggressive. Higher values reflect RFC 8985 §7.1's guidance
    /// that reordering-detected losses MAY use a gentler response since the data may
    /// have only been delayed. The default keeps RFC 4960 halving in both regimes;
    /// raise to e.g. 70 to opt in to the gentler reordering response.
    /// Default: 50.
    rack_recovery_cwnd_factor_percent: u8,

    /// Optional hard upper bound applied to `cwnd` after each congestion-control
    /// increment. `None` (default) preserves standard RFC 4960 behavior. `Some(N)`
    /// clamps the congestion window to at most `N` bytes, which is useful when a
    /// downstream pipe (e.g. a TURN-relay egress queue) cannot absorb the bursts
    /// produced by unbounded cwnd growth. Has no effect on RFC-defined reductions.
    /// Default: None.
    max_cwnd_bytes: Option<u32>,

    /// When true, RACK tracks per-association delivery jitter and dynamically expands
    /// `rack_reo_wnd` above `rack_reo_wnd_floor` to match the observed envelope.
    /// `rack_reo_wnd_floor` becomes a minimum below which the dynamic value never falls.
    /// When false, RACK uses `rack_reo_wnd_floor` exactly as before this feature was added.
    /// Default: true.
    rack_adaptive: bool,
}

impl Default for TransportConfig {
    fn default() -> Self {
        TransportConfig {
            max_receive_buffer_size: INITIAL_RECV_BUF_SIZE,
            max_send_message_size: DEFAULT_MAX_MESSAGE_SIZE,
            max_receive_message_size: DEFAULT_MAX_MESSAGE_SIZE,
            max_num_outbound_streams: u16::MAX,
            max_num_inbound_streams: u16::MAX,
            max_init_retransmits: Some(DEFAULT_MAX_INIT_RETRANS),
            max_data_retransmits: None,
            rto_initial_ms: RTO_INITIAL,
            rto_min_ms: RTO_MIN,
            rto_max_ms: RTO_MAX,
            rack_min_rtt_window: DEFAULT_RACK_MIN_RTT_WINDOW,
            rack_reo_wnd_floor: DEFAULT_RACK_REO_WND_FLOOR,
            rack_worst_case_delayed_ack: DEFAULT_RACK_WORST_CASE_DELAYED_ACK,
            rack_recovery_cwnd_factor_percent: DEFAULT_RACK_RECOVERY_CWND_FACTOR_PERCENT,
            max_cwnd_bytes: None,
            rack_adaptive: true,
        }
    }
}

impl TransportConfig {
    pub fn for_relay() -> Self {
        Self::default()
            .with_max_init_retransmits(None)
            .with_max_data_retransmits(None)
            .with_rack_reo_wnd_floor(Duration::from_millis(400))
            .with_rack_recovery_cwnd_factor_percent(70)
            .with_rto_min_ms(3000)
    }

    /// Validate configuration values that cannot be represented as type-level invariants.
    pub fn validate(&self) -> Result<(), TransportConfigError> {
        if self.rack_min_rtt_window.is_zero() {
            return Err(TransportConfigError::InvalidRackMinRttWindow);
        }
        if self.rack_worst_case_delayed_ack.is_zero() {
            return Err(TransportConfigError::InvalidRackWorstCaseDelayedAck);
        }
        if self.rack_recovery_cwnd_factor_percent == 0 || self.rack_recovery_cwnd_factor_percent > 100 {
            return Err(TransportConfigError::InvalidRackRecoveryCwndFactor);
        }
        if let Some(v) = self.max_cwnd_bytes {
            if v < MIN_MAX_CWND_BYTES {
                return Err(TransportConfigError::InvalidMaxCwndBytes);
            }
        }

        Ok(())
    }

    pub fn with_max_receive_buffer_size(mut self, value: u32) -> Self {
        self.max_receive_buffer_size = value;
        self
    }

    pub fn with_max_send_message_size(mut self, value: u32) -> Self {
        self.max_send_message_size = value;
        self
    }

    /// Set maximum size of messages we will accept
    pub fn with_max_receive_message_size(mut self, value: u32) -> Self {
        self.max_receive_message_size = value;
        self
    }

    #[deprecated(note = "Use with_max_send_message_size instead")]
    pub fn with_max_message_size(self, value: u32) -> Self {
        self.with_max_send_message_size(value)
    }

    pub fn with_max_num_outbound_streams(mut self, value: u16) -> Self {
        self.max_num_outbound_streams = value;
        self
    }

    pub fn with_max_num_inbound_streams(mut self, value: u16) -> Self {
        self.max_num_inbound_streams = value;
        self
    }

    pub(crate) fn max_receive_buffer_size(&self) -> u32 {
        self.max_receive_buffer_size
    }

    pub(crate) fn max_send_message_size(&self) -> u32 {
        self.max_send_message_size
    }

    pub(crate) fn max_receive_message_size(&self) -> u32 {
        self.max_receive_message_size
    }

    pub(crate) fn max_num_outbound_streams(&self) -> u16 {
        self.max_num_outbound_streams
    }

    pub(crate) fn max_num_inbound_streams(&self) -> u16 {
        self.max_num_inbound_streams
    }

    /// Set maximum INIT retransmissions. `None` means unlimited.
    pub fn with_max_init_retransmits(mut self, value: Option<usize>) -> Self {
        self.max_init_retransmits = value;
        self
    }

    /// Set maximum DATA retransmissions. `None` means unlimited.
    pub fn with_max_data_retransmits(mut self, value: Option<usize>) -> Self {
        self.max_data_retransmits = value;
        self
    }

    /// Set initial RTO in milliseconds.
    pub fn with_rto_initial_ms(mut self, value: u64) -> Self {
        self.rto_initial_ms = value;
        self
    }

    /// Set minimum RTO in milliseconds.
    pub fn with_rto_min_ms(mut self, value: u64) -> Self {
        self.rto_min_ms = value;
        self
    }

    /// Set maximum RTO in milliseconds.
    pub fn with_rto_max_ms(mut self, value: u64) -> Self {
        self.rto_max_ms = value;
        self
    }

    /// Set the window used to track the minimum RTT for RACK.
    pub fn with_rack_min_rtt_window(mut self, value: Duration) -> Self {
        self.rack_min_rtt_window = value;
        self
    }

    /// Set the floor applied to the derived RACK reordering window.
    pub fn with_rack_reo_wnd_floor(mut self, value: Duration) -> Self {
        self.rack_reo_wnd_floor = value;
        self
    }

    /// Set the worst-case delayed-ACK allowance used for single-packet PTO.
    pub fn with_rack_worst_case_delayed_ack(mut self, value: Duration) -> Self {
        self.rack_worst_case_delayed_ack = value;
        self
    }

    /// Set the multiplier (as a percentage in 1..=100) applied to cwnd when RACK
    /// marks a chunk lost and the association enters fast recovery.
    pub fn with_rack_recovery_cwnd_factor_percent(mut self, value: u8) -> Self {
        self.rack_recovery_cwnd_factor_percent = value;
        self
    }

    /// Set an optional hard upper bound for `cwnd` (in bytes). `None` preserves
    /// standard RFC 4960 unbounded growth. `Some(v)` requires `v >= 4380` and
    /// clamps `cwnd` after every congestion-control increment.
    pub fn with_max_cwnd_bytes(mut self, value: Option<u32>) -> Self {
        self.max_cwnd_bytes = value;
        self
    }

    pub(crate) fn max_init_retransmits(&self) -> Option<usize> {
        self.max_init_retransmits
    }

    pub(crate) fn max_data_retransmits(&self) -> Option<usize> {
        self.max_data_retransmits
    }

    pub(crate) fn rto_initial_ms(&self) -> u64 {
        self.rto_initial_ms
    }

    pub(crate) fn rto_min_ms(&self) -> u64 {
        self.rto_min_ms
    }

    pub(crate) fn rto_max_ms(&self) -> u64 {
        self.rto_max_ms
    }

    /// Get the configured RACK minimum RTT window.
    pub fn get_rack_min_rtt_window(&self) -> Duration {
        self.rack_min_rtt_window
    }

    /// Get the configured RACK reordering-window floor.
    pub fn get_rack_reo_wnd_floor(&self) -> Duration {
        self.rack_reo_wnd_floor
    }

    /// Get the configured worst-case delayed-ACK allowance used for single-packet PTO.
    pub fn get_rack_worst_case_delayed_ack(&self) -> Duration {
        self.rack_worst_case_delayed_ack
    }

    /// Get the configured RACK recovery cwnd factor (as a percentage in 1..=100).
    pub fn get_rack_recovery_cwnd_factor_percent(&self) -> u8 {
        self.rack_recovery_cwnd_factor_percent
    }

    /// Get the optional hard cap on `cwnd` in bytes.
    pub fn get_max_cwnd_bytes(&self) -> Option<u32> {
        self.max_cwnd_bytes
    }

    /// Toggle the adaptive RACK reordering window.
    pub fn with_rack_adaptive(mut self, value: bool) -> Self {
        self.rack_adaptive = value;
        self
    }

    /// Get whether the adaptive RACK reordering window is enabled.
    pub fn get_rack_adaptive(&self) -> bool {
        self.rack_adaptive
    }
}

/// Global configuration for the endpoint, affecting all associations
///
/// Default values should be suitable for most internet applications.
#[derive(Clone)]
pub struct EndpointConfig {
    pub(crate) max_payload_size: u32,

    /// AID generator factory
    ///
    /// Create a aid generator for local aid in Endpoint struct
    pub(crate) aid_generator_factory:
        Arc<dyn Fn() -> Box<dyn AssociationIdGenerator> + Send + Sync>,
}

impl Default for EndpointConfig {
    fn default() -> Self {
        Self::new()
    }
}

impl EndpointConfig {
    /// Create a default config
    pub fn new() -> Self {
        let aid_factory: fn() -> Box<dyn AssociationIdGenerator> =
            || Box::<RandomAssociationIdGenerator>::default();
        Self {
            max_payload_size: INITIAL_MTU - (COMMON_HEADER_SIZE + DATA_CHUNK_HEADER_SIZE),
            aid_generator_factory: Arc::new(aid_factory),
        }
    }

    /// Supply a custom Association ID generator factory
    ///
    /// Called once by each `Endpoint` constructed from this configuration to obtain the AID
    /// generator which will be used to generate the AIDs used for incoming packets on all
    /// associations involving that  `Endpoint`. A custom AID generator allows applications to embed
    /// information in local association IDs, e.g. to support stateless packet-level load balancers.
    ///
    /// `EndpointConfig::new()` applies a default random AID generator factory. This functions
    /// accepts any customized AID generator to reset AID generator factory that implements
    /// the `AssociationIdGenerator` trait.
    pub fn aid_generator<F: Fn() -> Box<dyn AssociationIdGenerator> + Send + Sync + 'static>(
        &mut self,
        factory: F,
    ) -> &mut Self {
        self.aid_generator_factory = Arc::new(factory);
        self
    }

    /// Maximum payload size accepted from peers.
    ///
    /// The default is suitable for typical internet applications. Applications which expect to run
    /// on networks supporting Ethernet jumbo frames or similar should set this appropriately.
    pub fn max_payload_size(&mut self, value: u32) -> &mut Self {
        self.max_payload_size = value;
        self
    }

    /// Get the current value of `max_payload_size`
    ///
    /// While most parameters don't need to be readable, this must be exposed to allow higher-level
    /// layers to determine how large a receive buffer to allocate to
    /// support an externally-defined `EndpointConfig`.
    ///
    /// While `get_` accessors are typically unidiomatic in Rust, we favor concision for setters,
    /// which will be used far more heavily.
    #[doc(hidden)]
    pub fn get_max_payload_size(&self) -> u32 {
        self.max_payload_size
    }
}

impl fmt::Debug for EndpointConfig {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt.debug_struct("EndpointConfig")
            .field("max_payload_size", &self.max_payload_size)
            .field("aid_generator_factory", &"[ elided ]")
            .finish()
    }
}

/// Parameters governing incoming associations
///
/// Default values should be suitable for most internet applications.
#[derive(Debug, Clone)]
pub struct ServerConfig {
    /// Transport configuration to use for incoming associations
    pub transport: Arc<TransportConfig>,

    /// Maximum number of concurrent associations
    pub(crate) concurrent_associations: u32,
}

impl Default for ServerConfig {
    fn default() -> Self {
        ServerConfig {
            transport: Arc::new(TransportConfig::default()),
            concurrent_associations: 100_000,
        }
    }
}

impl ServerConfig {
    /// Create a default config with a particular handshake token key
    pub fn new() -> Self {
        ServerConfig::default()
    }
}

/// Default SCTP source/destination port (conventional for WebRTC data channels).
pub const DEFAULT_SCTP_PORT: u16 = 5000;

/// Maximum allowed size (in bytes) of a serialized SNAP token (INIT chunk)
/// accepted via out-of-band negotiation. A typical token is well under
/// 100 bytes; this limit prevents accidentally feeding megabytes of
/// untrusted signaling data into the parser.
pub const MAX_SNAP_INIT_BYTES: usize = 2048;

/// Configuration for outgoing associations.
///
/// Default values should be suitable for most internet applications.
#[derive(Debug, Clone)]
pub struct ClientConfig {
    /// Transport configuration to use
    pub transport: Arc<TransportConfig>,
    /// Local SNAP token (INIT chunk) bytes.
    ///
    /// Generated via [`generate_snap_token`]. When both `local_sctp_init` and
    /// `remote_sctp_init` are set, the association skips the SCTP 4-way
    /// handshake (RFC 4960 Section 5.1) and immediately transitions to the
    /// ESTABLISHED state.
    ///
    /// If only one side is set (e.g. the peer does not support SNAP), the
    /// association falls back to the normal SCTP handshake.
    ///
    /// See [draft-hancke-tsvwg-snap-01](https://datatracker.ietf.org/doc/draft-hancke-tsvwg-snap/).
    pub(crate) local_sctp_init: Option<Bytes>,
    /// Remote SNAP token (INIT chunk) bytes.
    ///
    /// Received from the peer via a signaling channel (e.g., SDP `a=sctp-init`
    /// attribute). Must be provided together with `local_sctp_init` to enable
    /// SNAP.
    ///
    /// See [draft-hancke-tsvwg-snap-01](https://datatracker.ietf.org/doc/draft-hancke-tsvwg-snap/).
    pub(crate) remote_sctp_init: Option<Bytes>,
}

impl Default for ClientConfig {
    fn default() -> Self {
        ClientConfig {
            transport: Arc::new(TransportConfig::default()),
            local_sctp_init: None,
            remote_sctp_init: None,
        }
    }
}

impl ClientConfig {
    /// Create a default config with a particular cryptographic config
    pub fn new() -> Self {
        ClientConfig::default()
    }

    /// Enable SNAP (SCTP Negotiation Acceleration Protocol).
    ///
    /// Both a local and remote SNAP token (INIT chunk) must be provided.
    /// The local token should be generated via [`generate_snap_token`] and
    /// exchanged with the remote peer through a signaling channel (e.g.,
    /// SDP `a=sctp-init` attribute). The remote token is the peer's
    /// corresponding bytes received via signaling.
    ///
    /// When both are set, the association skips the SCTP 4-way handshake
    /// (RFC 4960 Section 5.1) and immediately transitions to the ESTABLISHED
    /// state.
    ///
    /// **Note:** When using SNAP, **both** peers must call
    /// [`Endpoint::connect`](crate::Endpoint::connect) — there is no
    /// server-side SNAP via [`Endpoint::handle`](crate::Endpoint::handle).
    ///
    /// See [draft-hancke-tsvwg-snap-01](https://datatracker.ietf.org/doc/draft-hancke-tsvwg-snap/).
    pub fn with_snap(mut self, local_sctp_init: Bytes, remote_sctp_init: Bytes) -> Self {
        self.local_sctp_init = Some(local_sctp_init);
        self.remote_sctp_init = Some(remote_sctp_init);
        self
    }
}

/// Generate a SNAP token (INIT chunk) for out-of-band negotiation.
///
/// Creates a serialized SCTP INIT **chunk** (not a full SCTP packet — no
/// common header or IP/UDP framing) with random `initiate_tag` and
/// `initial_tsn` values, using the receiver window from the provided
/// [`TransportConfig`]. Stream counts are set to `u16::MAX` so that the
/// actual limit is determined by the peer's offer during negotiation.
///
/// The returned bytes are suitable for exchange via a signaling channel
/// (e.g., SDP `a=sctp-init`) as described in
/// [draft-hancke-tsvwg-snap-01](https://datatracker.ietf.org/doc/draft-hancke-tsvwg-snap/).
///
/// Each call generates fresh random values. The caller must hold onto the
/// returned bytes and pass them to [`ClientConfig::with_snap`] alongside
/// the remote peer's token.
pub fn generate_snap_token(config: &TransportConfig) -> Result<Bytes, crate::error::Error> {
    use crate::chunk::{Chunk, chunk_init::ChunkInit};
    use core::num::NonZeroU32;
    use rand::random;

    let mut init = ChunkInit {
        initiate_tag: random::<NonZeroU32>().get(),
        initial_tsn: random::<NonZeroU32>().get(),
        num_outbound_streams: u16::MAX,
        num_inbound_streams: u16::MAX,
        advertised_receiver_window_credit: config.max_receive_buffer_size(),
        ..Default::default()
    };
    init.set_supported_extensions();
    init.check()?;
    init.marshal()
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_transport_config_for_relay_profile() {
        let config = TransportConfig::for_relay();

        assert_eq!(None, config.max_init_retransmits());
        assert_eq!(None, config.max_data_retransmits());
        assert_eq!(Duration::from_millis(400), config.get_rack_reo_wnd_floor());
        assert_eq!(None, config.get_max_cwnd_bytes());
        assert_eq!(70, config.get_rack_recovery_cwnd_factor_percent());
        assert_eq!(3000, config.rto_min_ms());
        assert!(config.get_rack_adaptive());
        assert_eq!(Ok(()), config.validate());
    }

    #[test]
    fn test_transport_config_rack_adaptive_default_and_override() {
        let default_cfg = TransportConfig::default();
        assert!(default_cfg.get_rack_adaptive());

        let opted_out = TransportConfig::default().with_rack_adaptive(false);
        assert!(!opted_out.get_rack_adaptive());

        let relay_opted_out = TransportConfig::for_relay().with_rack_adaptive(false);
        assert!(!relay_opted_out.get_rack_adaptive());
        assert_eq!(
            Duration::from_millis(400),
            relay_opted_out.get_rack_reo_wnd_floor()
        );
    }

    #[test]
    fn test_transport_config_rack_defaults() {
        let config = TransportConfig::default();

        assert_eq!(DEFAULT_RACK_MIN_RTT_WINDOW, config.get_rack_min_rtt_window());
        assert_eq!(DEFAULT_RACK_REO_WND_FLOOR, config.get_rack_reo_wnd_floor());
        assert_eq!(
            DEFAULT_RACK_WORST_CASE_DELAYED_ACK,
            config.get_rack_worst_case_delayed_ack()
        );
        assert_eq!(
            DEFAULT_RACK_RECOVERY_CWND_FACTOR_PERCENT,
            config.get_rack_recovery_cwnd_factor_percent()
        );
        assert_eq!(None, config.get_max_cwnd_bytes());
        assert_eq!(Ok(()), config.validate());
    }

    #[test]
    fn test_transport_config_max_cwnd_bytes_overrides() {
        let cfg_none = TransportConfig::default().with_max_cwnd_bytes(None);
        assert_eq!(None, cfg_none.get_max_cwnd_bytes());
        assert_eq!(Ok(()), cfg_none.validate());

        let cfg_some = TransportConfig::default().with_max_cwnd_bytes(Some(200_000));
        assert_eq!(Some(200_000), cfg_some.get_max_cwnd_bytes());
        assert_eq!(Ok(()), cfg_some.validate());

        let cfg_min = TransportConfig::default().with_max_cwnd_bytes(Some(MIN_MAX_CWND_BYTES));
        assert_eq!(Ok(()), cfg_min.validate());

        let cfg_too_low =
            TransportConfig::default().with_max_cwnd_bytes(Some(MIN_MAX_CWND_BYTES - 1));
        assert_eq!(
            Err(TransportConfigError::InvalidMaxCwndBytes),
            cfg_too_low.validate()
        );
    }

    #[test]
    fn test_transport_config_rack_overrides() {
        let config = TransportConfig::default()
            .with_rack_min_rtt_window(Duration::from_secs(5))
            .with_rack_reo_wnd_floor(Duration::from_millis(3))
            .with_rack_worst_case_delayed_ack(Duration::from_millis(75))
            .with_rack_recovery_cwnd_factor_percent(50);

        assert_eq!(Duration::from_secs(5), config.get_rack_min_rtt_window());
        assert_eq!(Duration::from_millis(3), config.get_rack_reo_wnd_floor());
        assert_eq!(
            Duration::from_millis(75),
            config.get_rack_worst_case_delayed_ack()
        );
        assert_eq!(50, config.get_rack_recovery_cwnd_factor_percent());
        assert_eq!(Ok(()), config.validate());
    }

    #[test]
    fn test_transport_config_rack_validation() {
        let invalid_min_rtt = TransportConfig::default().with_rack_min_rtt_window(Duration::ZERO);
        assert_eq!(
            Err(TransportConfigError::InvalidRackMinRttWindow),
            invalid_min_rtt.validate()
        );

        let invalid_del_ack =
            TransportConfig::default().with_rack_worst_case_delayed_ack(Duration::ZERO);
        assert_eq!(
            Err(TransportConfigError::InvalidRackWorstCaseDelayedAck),
            invalid_del_ack.validate()
        );

        let invalid_factor_zero =
            TransportConfig::default().with_rack_recovery_cwnd_factor_percent(0);
        assert_eq!(
            Err(TransportConfigError::InvalidRackRecoveryCwndFactor),
            invalid_factor_zero.validate()
        );

        let invalid_factor_too_high =
            TransportConfig::default().with_rack_recovery_cwnd_factor_percent(101);
        assert_eq!(
            Err(TransportConfigError::InvalidRackRecoveryCwndFactor),
            invalid_factor_too_high.validate()
        );

        let valid_boundary =
            TransportConfig::default().with_rack_recovery_cwnd_factor_percent(100);
        assert_eq!(Ok(()), valid_boundary.validate());
    }
}
