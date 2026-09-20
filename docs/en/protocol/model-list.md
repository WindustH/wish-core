# Model list

`src/protocol/model_list/` is the intermediate layer for what models a service says it offers: it
receives one page of a catalog and converts it into `ModelCatalog`, so a caller can offer a model
picker without knowing which service answered. Ids are kept as the service spelled them - only the
`publishers/<publisher>/models/` prefix a Gemini resource name wraps around them is stripped,
because the wire wants the bare id back - a cursor only ever comes from the service, and what
cannot be represented is kept as warnings.

```
caller                      model_list                     service
──────────────────────────────────────────────────────────────────────────────
ModelCatalog ◀── parse_catalog_page() ◀── model_list/<dialect>.rs ◀── one page ◀── HTTP
              ── fetch() + build_page_query() ──▶ outbound (the entry's target × Credentials) ──▶ HTTP
```
