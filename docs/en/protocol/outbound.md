# Outbound

`src/protocol/outbound/` is the half every request shares: what a caller hands over to reach an
account, where a call goes, and the join that turns a tree's rendered request into the one call a
transport performs.
```
the trees                  outbound                              transport
──────────────────────────────────────────────────────────────────────────
a Draft ────────┐
an Outbound ────┼──▶ dispatch ──▶ Call ───────────────────────────▶ HTTP
Credentials ────┘

Credentials ──▶ expired ──▶ bool (the judgment `dispatch` refuses by)
a credential ─▶ oauth ──▶ a new Tokens pair (dispatched like any call)
AWS material ─▶ sigv4 ──▶ three signed headers, written last
an ADC file ──▶ adc ──▶ material: refresh (a login) or sign (a key)
```
