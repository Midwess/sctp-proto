# loss-demo

Standalone example crate that depends on the local `sctp-proto` checkout and drives two
in-memory SCTP peers through:

- client-to-server early packet loss, which should trigger RACK loss marking
- server-to-client tail loss, which should trigger PTO recovery

Run it with:

```bash
rtk cargo run --manifest-path examples/loss-demo/Cargo.toml
```
