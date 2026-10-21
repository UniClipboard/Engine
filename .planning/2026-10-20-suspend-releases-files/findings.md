# Findings

See the thread report for the final summary.

## E3 startup drain

Single-device run, no peers, macOS, product relay defaults (detailed network capture):

| suspend at (ms after start) | suspend took (ms) |
|---|---|
| 752 | 4005 |
| 3693 | 2677 |
| 10704 | 181 |

Network log shows 3 QUIC handshakes at ~0.75-0.95 s (no peer exists, so these are iroh
net_report QAD probes to relays); two are closed at ~1.1-1.25 s (probe cancellation) and only
reach `drained` ~5.1 s later. The suspend returns in the same millisecond as the last `drained`.
`Endpoint::close` -> `wait_all_draining` waits for them. Router stops protocol handlers (blob
store) before closing the endpoint, so the drain itself holds only the UDP socket, but the
lifecycle order releases DB/vault/network-log files only after session work (incl. drain) ends.
