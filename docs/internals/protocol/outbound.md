# Outbound

`src/protocol/outbound/` is the half every request shares: what is handed over to reach an account,
where a call goes, and the join that turns a tree's rendered request into the one call a transport
performs.

```
the trees                  outbound                              transport
──────────────────────────────────────────────────────────────────────────
a Draft ────────┐
an Outbound ────┼──▶ dispatch ──▶ Call ───────────────────────────▶ HTTP
Credentials ────┘

Credentials ──▶ is_expired(material, now) ──▶ bool (the judgment `dispatch` refuses by)
a refresh token ─▶ oauth ──▶ Tokens (one attempt, no retry)
AWS material ─▶ sigv4 ──▶ x-amz-date, authorization, and x-amz-security-token
                          with a session token; written last
an ADC file ──▶ adc ──▶ an access token: refresh (a login) or an RS256 assertion (a key)
```

## Types

`AuthProtocol` states where the credential sits:

| Variant | Placement |
| --- | --- |
| `None` | nothing |
| `Bearer(Option<CredentialsRefreshProtocol>)` | `Authorization: Bearer <api_key>`; the option names how the token renews |
| `Header(&'static str)` | the key in one named header (`x-api-key`, `x-goog-api-key`) |
| `SigV4` | an AWS signature over the finished request, service `bedrock` |

`CredentialsRefreshProtocol` is `OAuth` (the Codex subscription's refresh token) or
`GoogleAdc { adc, scope }`. The server builds `Bearer(Some(OAuth))` for the `openai_codex` preset
and never builds `GoogleAdc` (`src/server/provider.rs`).

`Outbound::new(base_url, path, auth)` requires an absolute http(s) URL and a non-empty path.
`with_header` adds static headers, `with_credential_header` places a credential field in a named
header, and `with_session_id` binds the value for `{session}` header templates. `resolve_path`
fills `{model}`, percent-encoded.

`Credentials` holds `api_key`, `workspace_id`, `team_id`, `organization`, `project`,
`account_id`, `refresh_token`, `expires_at` and the AWS `region`, `access_key_id`,
`secret_access_key`, `session_token`. `Credentials::renew(&Tokens)` applies an exchange's result.
`Tokens` is `{ access_token, refresh_token, id_token, account_id, expires_at }`.

`Draft` is a tree's rendered request: `{ method, path, query, headers, body }`.

## Dispatch order

1. Refuse with `Error::Renewal` if `expires_at` has passed.
2. Fill `{field}` placeholders in the base URL and the path from the material, percent-encoded:
   `api_key`, `workspace_id`, `team_id`, `organization`, `project`, `account_id`, `region`. This
   is how a Bedrock or Qwen workspace host carries its region or workspace. AWS secrets are never
   placeable.
3. Append the query, percent-encoded, in wire order.
4. Merge headers: static, with `{session}` expanded (a fresh UUID if unbound), then auth, then
   credential headers, then the draft's own. Later entries replace earlier ones case-insensitively,
   so a protocol can always override configuration.
5. For `SigV4`, sign last over the request exactly as it stands. The signature uses the system
   clock.

Credentials never travel in a URL.

## Read sources and exchanges

`table.rs` defines `Source` and `SourceHeader`: the static rows that
[account-state](account-state.md#sources) and [model-list](model-list.md#protocols) read from.
A row becomes an `Outbound` for one GET with no body. The caller's `base_url`/`path` override the
row's, and a row with neither is `Error::Build`.

- `oauth`: POSTs JSON to `https://auth.openai.com/oauth/token` with the Codex CLI client id.
  `account_id` comes from the id-token claim, and `expires_at` from the access-token JWT, falling
  back to `expires_in`. Debug builds honor `WISH_TEST_CODEX_ISSUER`.
- `adc`: `parse` accepts `authorized_user` and `service_account` files and refuses
  `external_account`. A service account signs an RS256 assertion valid for 3600 s. The token
  response must carry `expires_in`.

Both exchanges are one attempt, and deciding when to refresh stays with the caller.
