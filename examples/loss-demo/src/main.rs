use bytes::Bytes;
use sctp_proto::{
    Association, AssociationHandle, AssociationStats, ClientConfig, DatagramEvent, Endpoint,
    EndpointConfig, Event, Payload, PayloadProtocolIdentifier, ServerConfig, StreamEvent, Transmit,
    TransportConfig,
};
use std::collections::{HashMap, HashSet, VecDeque};
use std::error::Error;
use std::net::{Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};

const STREAM_ID: u16 = 7;
const TICK: Duration = Duration::from_millis(10);
const LINK_LATENCY: Duration = Duration::from_millis(20);
const DEMO_RACK_MIN_RTT_WINDOW: Duration = Duration::from_secs(5);
const DEMO_RACK_REO_WND_FLOOR: Duration = Duration::from_millis(5);
const DEMO_RACK_WORST_CASE_DELAYED_ACK: Duration = Duration::from_millis(75);

#[derive(Debug)]
struct QueuedDatagram {
    deliver_at: Instant,
    bytes: Bytes,
}

#[derive(Clone, Copy, Debug, Default)]
struct StatsSnapshot {
    sacks: u64,
    t3_timeouts: u64,
    rack_loss_marks: u64,
    pto_timeouts: u64,
}

impl StatsSnapshot {
    fn capture(association: &Association) -> Self {
        let mut stats: AssociationStats = association.stats();
        Self {
            sacks: stats.get_num_sacks(),
            t3_timeouts: stats.get_num_t3timeouts(),
            rack_loss_marks: stats.get_num_rack_loss_marks(),
            pto_timeouts: stats.get_num_pto_timeouts(),
        }
    }

    fn print_delta(self, baseline: Self, label: &str) {
        println!(
            "{label}: sacks=+{} rack_loss_marks=+{} pto_timeouts=+{} t3_timeouts=+{}",
            self.sacks.saturating_sub(baseline.sacks),
            self.rack_loss_marks
                .saturating_sub(baseline.rack_loss_marks),
            self.pto_timeouts.saturating_sub(baseline.pto_timeouts),
            self.t3_timeouts.saturating_sub(baseline.t3_timeouts),
        );
    }
}

struct Peer {
    name: &'static str,
    addr: SocketAddr,
    endpoint: Endpoint,
    connect_config: Option<ClientConfig>,
    associations: HashMap<AssociationHandle, Association>,
    accepted: Option<AssociationHandle>,
    pending_assoc_events: HashMap<AssociationHandle, VecDeque<sctp_proto::AssociationEvent>>,
    connected: HashSet<AssociationHandle>,
}

impl Peer {
    fn new_client(addr: SocketAddr, transport: Arc<TransportConfig>) -> Self {
        let mut connect_config = ClientConfig::default();
        connect_config.transport = transport;
        Self {
            name: "client",
            addr,
            endpoint: Endpoint::new(Arc::new(EndpointConfig::default()), None),
            connect_config: Some(connect_config),
            associations: HashMap::new(),
            accepted: None,
            pending_assoc_events: HashMap::new(),
            connected: HashSet::new(),
        }
    }

    fn new_server(addr: SocketAddr, transport: Arc<TransportConfig>) -> Self {
        let mut server_config = ServerConfig::default();
        server_config.transport = transport;
        Self {
            name: "server",
            addr,
            endpoint: Endpoint::new(
                Arc::new(EndpointConfig::default()),
                Some(Arc::new(server_config)),
            ),
            connect_config: None,
            associations: HashMap::new(),
            accepted: None,
            pending_assoc_events: HashMap::new(),
            connected: HashSet::new(),
        }
    }

    fn begin_connect(&mut self, remote: SocketAddr) -> Result<AssociationHandle, Box<dyn Error>> {
        let client_config = self
            .connect_config
            .clone()
            .ok_or("client transport configuration is missing")?;
        let (handle, association) = self.endpoint.connect(client_config, remote)?;
        self.associations.insert(handle, association);
        Ok(handle)
    }

    fn ingest(&mut self, now: Instant, remote: SocketAddr, bytes: Bytes) {
        if let Some((handle, event)) = self.endpoint.handle(now, remote, None, None, bytes) {
            match event {
                DatagramEvent::NewAssociation(association) => {
                    self.accepted = Some(handle);
                    self.associations.insert(handle, association);
                }
                DatagramEvent::AssociationEvent(event) => {
                    self.pending_assoc_events
                        .entry(handle)
                        .or_default()
                        .push_back(event);
                }
            }
        }
    }

    fn collect_transmits(&mut self, now: Instant) -> Vec<Transmit> {
        let mut outbound = Vec::new();

        while let Some(transmit) = self.endpoint.poll_transmit() {
            outbound.push(transmit);
        }

        let handles: Vec<_> = self.associations.keys().copied().collect();
        for handle in handles {
            let endpoint_events = {
                let Some(association) = self.associations.get_mut(&handle) else {
                    continue;
                };

                if let Some(events) = self.pending_assoc_events.get_mut(&handle) {
                    while let Some(event) = events.pop_front() {
                        association.handle_event(event);
                    }
                }

                while association
                    .poll_timeout()
                    .is_some_and(|deadline| deadline <= now)
                {
                    association.handle_timeout(now);
                }

                let mut endpoint_events = Vec::new();
                while let Some(event) = association.poll_endpoint_event() {
                    endpoint_events.push(event);
                }

                while let Some(transmit) = association.poll_transmit(now) {
                    outbound.push(transmit);
                }

                while let Some(event) = association.poll() {
                    match event {
                        Event::Connected => {
                            self.connected.insert(handle);
                        }
                        Event::HandshakeFailed { reason } => {
                            println!("[{}] handshake failed: {reason}", self.name);
                        }
                        Event::AssociationLost { id, reason } => {
                            println!("[{}] association lost on stream {id}: {reason}", self.name);
                        }
                        Event::Stream(StreamEvent::Opened { id }) => {
                            println!("[{}] stream {id} opened", self.name);
                        }
                        Event::Stream(StreamEvent::Readable { id }) => {
                            println!("[{}] stream {id} readable", self.name);
                        }
                        Event::Stream(StreamEvent::Writable { id }) => {
                            println!("[{}] stream {id} writable", self.name);
                        }
                        Event::Stream(StreamEvent::Finished { id }) => {
                            println!("[{}] stream {id} finished", self.name);
                        }
                        Event::Stream(StreamEvent::Stopped { id, error_code }) => {
                            println!("[{}] stream {id} stopped: {error_code}", self.name);
                        }
                        Event::Stream(StreamEvent::Available) => {}
                        Event::Stream(StreamEvent::BufferedAmountLow { .. }) => {}
                        Event::Stream(StreamEvent::BufferedAmountHigh { .. }) => {}
                        Event::DatagramReceived => {}
                        _ => {}
                    }
                }
                endpoint_events
            };

            for event in endpoint_events {
                let Some(association) = self.associations.get_mut(&handle) else {
                    continue;
                };
                if let Some(assoc_event) = self.endpoint.handle_event(handle, event) {
                    association.handle_event(assoc_event);
                }
            }
        }

        outbound
    }

    fn association(&self, handle: AssociationHandle) -> &Association {
        self.associations
            .get(&handle)
            .unwrap_or_else(|| panic!("missing association {:?} on {}", handle, self.name))
    }

    fn association_mut(&mut self, handle: AssociationHandle) -> &mut Association {
        self.associations
            .get_mut(&handle)
            .unwrap_or_else(|| panic!("missing association {:?} on {}", handle, self.name))
    }
}

struct Lab {
    now: Instant,
    latency: Duration,
    client: Peer,
    server: Peer,
    client_to_server: VecDeque<QueuedDatagram>,
    server_to_client: VecDeque<QueuedDatagram>,
    drop_client_to_server: usize,
    drop_server_to_client: usize,
}

impl Lab {
    fn new(client_transport: Arc<TransportConfig>, server_transport: Arc<TransportConfig>) -> Self {
        let client_addr = SocketAddr::new(Ipv6Addr::LOCALHOST.into(), 5000);
        let server_addr = SocketAddr::new(Ipv6Addr::LOCALHOST.into(), 5001);

        Self {
            now: Instant::now(),
            latency: LINK_LATENCY,
            client: Peer::new_client(client_addr, client_transport),
            server: Peer::new_server(server_addr, server_transport),
            client_to_server: VecDeque::new(),
            server_to_client: VecDeque::new(),
            drop_client_to_server: 0,
            drop_server_to_client: 0,
        }
    }

    fn connect(&mut self) -> Result<(AssociationHandle, AssociationHandle), Box<dyn Error>> {
        let client_handle = self.client.begin_connect(self.server.addr)?;
        let client_deadline = self.now + Duration::from_secs(3);
        while self.now <= client_deadline
            && !(self.client.connected.contains(&client_handle) && self.server.accepted.is_some())
        {
            self.step();
        }
        if !(self.client.connected.contains(&client_handle) && self.server.accepted.is_some()) {
            return Err("timed out waiting for client connection".into());
        }

        let server_handle = self.server.accepted.expect("server should accept");
        let server_deadline = self.now + Duration::from_secs(1);
        while self.now <= server_deadline && !self.server.connected.contains(&server_handle) {
            self.step();
        }
        if !self.server.connected.contains(&server_handle) {
            return Err("timed out waiting for server connection".into());
        }

        Ok((client_handle, server_handle))
    }

    fn step(&mut self) {
        self.deliver_due_datagrams();

        let client_tx = self.client.collect_transmits(self.now);
        let server_tx = self.server.collect_transmits(self.now);

        self.enqueue_transmits(true, client_tx);
        self.enqueue_transmits(false, server_tx);

        self.now += TICK;
    }

    fn drive_for(&mut self, duration: Duration) {
        let deadline = self.now + duration;
        while self.now <= deadline {
            self.step();
        }
    }

    fn deliver_due_datagrams(&mut self) {
        while self
            .client_to_server
            .front()
            .is_some_and(|queued| queued.deliver_at <= self.now)
        {
            let queued = self.client_to_server.pop_front().expect("checked front");
            self.server.ingest(self.now, self.client.addr, queued.bytes);
        }

        while self
            .server_to_client
            .front()
            .is_some_and(|queued| queued.deliver_at <= self.now)
        {
            let queued = self.server_to_client.pop_front().expect("checked front");
            self.client.ingest(self.now, self.server.addr, queued.bytes);
        }
    }

    fn enqueue_transmits(&mut self, from_client: bool, transmits: Vec<Transmit>) {
        for transmit in transmits {
            let Payload::RawEncode(contents) = transmit.payload else {
                continue;
            };

            for bytes in contents {
                if from_client {
                    if self.drop_client_to_server > 0 {
                        self.drop_client_to_server -= 1;
                        println!(
                            "[network] dropped client -> server datagram ({} bytes)",
                            bytes.len()
                        );
                        continue;
                    }

                    self.client_to_server.push_back(QueuedDatagram {
                        deliver_at: self.now + self.latency,
                        bytes,
                    });
                } else {
                    if self.drop_server_to_client > 0 {
                        self.drop_server_to_client -= 1;
                        println!(
                            "[network] dropped server -> client datagram ({} bytes)",
                            bytes.len()
                        );
                        continue;
                    }

                    self.server_to_client.push_back(QueuedDatagram {
                        deliver_at: self.now + self.latency,
                        bytes,
                    });
                }
            }
        }
    }

    fn read_all_messages(
        peer: &mut Peer,
        handle: AssociationHandle,
        stream_id: u16,
    ) -> Result<Vec<String>, Box<dyn Error>> {
        let mut stream = peer.association_mut(handle).stream(stream_id)?;
        let mut messages = Vec::new();

        while let Some(chunks) = stream.read_sctp()? {
            let mut buf = vec![0; chunks.len()];
            let n = chunks.read(&mut buf)?;
            messages.push(String::from_utf8(buf[..n].to_vec())?);
        }

        Ok(messages)
    }
}

fn demo_transport_config() -> Result<Arc<TransportConfig>, Box<dyn Error>> {
    let config = TransportConfig::default()
        .with_rack_min_rtt_window(DEMO_RACK_MIN_RTT_WINDOW)
        .with_rack_reo_wnd_floor(DEMO_RACK_REO_WND_FLOOR)
        .with_rack_worst_case_delayed_ack(DEMO_RACK_WORST_CASE_DELAYED_ACK);
    config.validate()?;
    Ok(Arc::new(config))
}

fn print_transport_config(label: &str, config: &TransportConfig) {
    println!(
        "{label}: rack_min_rtt_window={:?} rack_reo_wnd_floor={:?} rack_worst_case_delayed_ack={:?}",
        config.get_rack_min_rtt_window(),
        config.get_rack_reo_wnd_floor(),
        config.get_rack_worst_case_delayed_ack(),
    );
}

fn main() -> Result<(), Box<dyn Error>> {
    let client_transport = demo_transport_config()?;
    let server_transport = demo_transport_config()?;
    print_transport_config("client transport", client_transport.as_ref());
    print_transport_config("server transport", server_transport.as_ref());

    let mut lab = Lab::new(client_transport, server_transport);
    let (client_handle, server_handle) = lab.connect()?;

    {
        let mut stream = lab
            .client
            .association_mut(client_handle)
            .open_stream(STREAM_ID, PayloadProtocolIdentifier::Binary)?;
        stream.write_sctp(
            &Bytes::from_static(b"bootstrap-from-client"),
            PayloadProtocolIdentifier::Binary,
        )?;
    }

    let stream_deadline = lab.now + Duration::from_secs(1);
    while lab.now <= stream_deadline
        && !lab
            .server
            .association(server_handle)
            .stream_ids()
            .contains(&STREAM_ID)
    {
        lab.step();
    }
    if !lab
        .server
        .association(server_handle)
        .stream_ids()
        .contains(&STREAM_ID)
    {
        return Err("timed out waiting for server stream".into());
    }

    let accepted_stream_id = {
        let stream = lab
            .server
            .association_mut(server_handle)
            .accept_stream()
            .ok_or("server did not expose an accepted stream")?;
        stream.stream_identifier()
    };
    assert_eq!(accepted_stream_id, STREAM_ID);

    let bootstrap = Lab::read_all_messages(&mut lab.server, server_handle, STREAM_ID)?;
    println!("bootstrap delivered to server: {bootstrap:?}");

    println!("\nphase 1: client -> server early loss, expecting RACK marking");
    let client_before = StatsSnapshot::capture(lab.client.association(client_handle));
    lab.drop_client_to_server = 1;

    for value in ["c-msg-0", "c-msg-1", "c-msg-2", "c-msg-3"] {
        {
            let mut stream = lab
                .client
                .association_mut(client_handle)
                .stream(STREAM_ID)?;
            stream.write_sctp(
                &Bytes::copy_from_slice(value.as_bytes()),
                PayloadProtocolIdentifier::Binary,
            )?;
        }
        lab.step();
    }

    lab.drive_for(Duration::from_secs(3));

    let mut server_messages = Lab::read_all_messages(&mut lab.server, server_handle, STREAM_ID)?;
    server_messages.retain(|message| message.starts_with("c-msg-"));
    println!("server received after loss recovery: {server_messages:?}");

    let client_after = StatsSnapshot::capture(lab.client.association(client_handle));
    client_after.print_delta(client_before, "client sender stats");

    println!("\nphase 2: server -> client tail loss, expecting PTO");
    let server_before = StatsSnapshot::capture(lab.server.association(server_handle));
    lab.drop_server_to_client = 1;

    {
        let mut stream = lab
            .server
            .association_mut(server_handle)
            .stream(STREAM_ID)?;
        stream.write_sctp(
            &Bytes::from_static(b"server-tail-loss"),
            PayloadProtocolIdentifier::Binary,
        )?;
    }

    lab.drive_for(Duration::from_secs(3));

    let client_messages = Lab::read_all_messages(&mut lab.client, client_handle, STREAM_ID)?;
    println!("client received after tail-loss recovery: {client_messages:?}");

    let server_after = StatsSnapshot::capture(lab.server.association(server_handle));
    server_after.print_delta(server_before, "server sender stats");

    println!(
        "\nfinal RTT estimates: client={:?} server={:?}",
        lab.client.association(client_handle).rtt(),
        lab.server.association(server_handle).rtt()
    );

    Ok(())
}
