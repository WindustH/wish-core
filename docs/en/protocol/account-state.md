# Account state

`src/protocol/account_state/` is the intermediate layer for what a service says about the account
behind a key: it receives a body a service reported and converts it into `AccountState`, so a caller
can show or act on usage without knowing which service produced it.

```
caller                      account                        service
──────────────────────────────────────────────────────────────────────────────
AccountState ◀── parse_account_body() ◀── account_state/<service>.rs ◀── a body ◀── HTTP
                 ◀── parse_headers()      ◀── headers.rs      ◀── reply head
                 ── fetch() ──▶ outbound (the entry's target × Credentials) ──▶ the read's own call ──▶ HTTP
```
