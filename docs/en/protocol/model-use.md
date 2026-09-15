# Model use

`src/protocol/model_use/` is an intermediate layer, and the home of `ModelUseProtocol` - the enum
of wires a conversation can speak, one variant per protocol kind with the compat mode that says
which vendor's reasoning extension rides it: it receives a request in our intermediate
representation and converts it into the form an upstream API accepts, and it receives an upstream
response or stream and converts it into the corresponding intermediate representation.

```
caller                      model_use                    wire
──────────────────────────────────────────────────────────────────────────────
Request      ──render()──▶  request/<dialect>.rs  ──▶  JSON body     ──▶  HTTP
Response     ◀──decode()──  response/<dialect>.rs ◀──  JSON body     ◀──  HTTP
StreamEvent  ◀──feed()────  stream/<dialect>.rs   ◀──  wire records  ◀──  SSE
```

