# Market observer groundwork

This branch adds an opt-in, privacy-bounded observation stream to HS Tracker's
existing passive capture and TCP reassembly pipeline. It is a protocol research
tool, not a market client: it performs no searches, sends no requests, and does
not touch the Hero Siege process.

## What it records

When `HS_MARKET_OBSERVER=1` is present at startup, recognized market-shaped
messages append one JSON object per line to `market-observations.jsonl` beside
the executable. Each record contains:

- timestamp plus opaque flow and adapter tags, salted for one tracker process;
- one-second packet/byte summaries for port 443 plus a measured count of
  TLS-like record headers, with no payload bytes saved;
- inferred client/server direction;
- a sanitized route path when present;
- a safe item name when an outbound auction post contains one;
- recognized structural field names; and
- the *names* of sensitive fields that were removed.

It never records values for identifiers, checksums, account IDs, fingerprints,
item hashes, masks, query text, prices, currencies, dynamic route segments, or
item blobs. Port-443 records are reduced to packet and byte counts without
copying their payload. Raw payloads are not written by this observer. Observer
mode forcibly disables HS Tracker's separate raw Debug Log even if that setting
was persisted as on.

The flow and adapter tags let records from one controlled experiment be grouped
without retaining an IP address, port tuple, or adapter GUID. They deliberately
change after the tracker restarts. The active log rotates at 16 MB and keeps one
older file, bounding the observer to roughly 32 MB on disk.

While enabled, the packet filter is restricted to exact remote address/port
pairs currently owned by the Hero Siege process, plus any endpoint explicitly
provided through `HS_MARKET_ENDPOINTS`. The tracker's normal `/24` expansion
and its all-TCP troubleshooting mode are not used. This is endpoint-level
attribution, not OS-level attribution of every packet: another application
talking to exactly the same host and port could match the filter.

## Controlled experiment

Npcap is required on Windows. Start the development build from an elevated
terminal with the observer enabled:

```powershell
$env:HS_MARKET_OBSERVER = '1'
$env:HS_MARKET_ENDPOINTS = 'market.example.invalid:443'
npm start
```

`HS_MARKET_ENDPOINTS` is optional. Use it only for one or more comma-separated
`HOST:PORT` values already established by the metadata phase. It closes the
race where a short-lived Search connection appears between process-socket
sweeps; names are resolved once at observer startup. Replace the reserved
example hostname above with the endpoint you established locally.

Then perform this sequence manually, noting the wall-clock time of each action:

1. Stay idle for 30 seconds.
2. Open the player marketplace.
3. Search once for a distinctive item.
4. Change exactly one filter and search again.
5. Move to the next results page once.
6. Close the marketplace.

Quit the tracker before reading `market-observations.jsonl`. Although the log is
sanitized and does not retain raw network addresses, review any diagnostic file
before sharing it. A useful passive result is any server message with a market
route or listing structure,
including plaintext relayed over port 443. A port number alone is not treated
as proof of TLS. If action-correlated 443 windows contain TLS-like record
headers and no parsed listing structure appears, the listing body is probably
encrypted and this branch must stop at metadata rather than attempting MITM,
certificate-pinning bypass, or authenticated request replay.

## Tests

```powershell
npm test
```

Tests prove that known Auction House post shapes are recognized while secret
values never enter serialized observations. They also cover route extraction
from `market/...?...` form messages.
