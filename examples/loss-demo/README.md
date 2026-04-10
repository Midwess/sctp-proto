# loss-demo

Standalone example crate that depends on the local `sctp-proto` checkout and drives two
in-memory SCTP peers through:

- client-to-server early packet loss, which should trigger RACK loss marking
- server-to-client tail loss, which should trigger PTO recovery

It also shows the current public RACK transport tuning API on both sides:

```rust
let transport = Arc::new(
    TransportConfig::default()
        .with_rack_min_rtt_window(Duration::from_secs(5))
        .with_rack_reo_wnd_floor(Duration::from_millis(5))
        .with_rack_worst_case_delayed_ack(Duration::from_millis(75)),
);

let mut client = ClientConfig::default();
client.transport = transport.clone();

let mut server = ServerConfig::default();
server.transport = transport;
```

The example validates the transport config before using it and prints the selected values at
startup, so it can be used as a reference for wiring custom RACK settings into
`ClientConfig` and `ServerConfig`.

Run it with:

```bash
rtk cargo run --manifest-path examples/loss-demo/Cargo.toml
```
