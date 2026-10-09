# Endpoint

`src/protocol/endpoint/` is the half every request shares: what is handed over to reach an account,
where a call goes, and the join that turns a tree's rendered request into the one call a transport
performs.

```
the trees                  endpoint                              transport
──────────────────────────────────────────────────────────────────────────
a Draft ────────┐
an Endpoint ────┼──▶ build_call ──▶ Call ─────────────────────────▶ HTTP
Credentials ────┘

a refresh token ──▶ codex_oauth ──▶ Tokens (one attempt, no retry)
a GitHub token ──▶ copilot_oauth ──▶ Tokens (one attempt, no retry)
AWS credentials ──▶ sigv4 ──▶ x-amz-date, authorization, and x-amz-security-token
                              with a session token; written last
```

## Types

`AuthScheme` states where the credential sits:

| Variant | Placement |
| --- | --- |
| `None` | nothing |
| `Bearer(Option<CredentialRenewal>)` | `Authorization: Bearer <api_key>`; the option names how the token renews |
| `Header(&'static str)` | the key in one named header (`x-api-key`, `x-goog-api-key`) |
| `SigV4` | an AWS signature over the finished request, service `bedrock` |
| `GitHubToken` | `Authorization: token <refresh_token>`: the GitHub token a Copilot session is exchanged from |

`CredentialRenewal` has two variants: `CodexOAuth` (the Codex subscription's refresh token) and
`CopilotToken` (a GitHub token exchanged for a Copilot session). The server builds
`Bearer(Some(CodexOAuth))` for the `openai_codex` preset and `Bearer(Some(CopilotToken))` for
`github_copilot` (`src/server/provider.rs`).

`Endpoint::new(base_url, path, auth)` requires an absolute http(s) URL and a non-empty path.
`with_header` adds static headers, `with_credential_header` places a credential field in a named
header, and `with_session_id` binds the value for `{session}` header templates. `resolve_path`
fills `{model}`, percent-encoded.

`Credentials` holds `api_key`, `workspace_id`, `team_id`, `organization`, `project`,
`account_id`, `refresh_token`, `expires_at` and the AWS `region`, `access_key_id`,
`secret_access_key`, `session_token`. `CredentialField` names each field the way configuration and
a path placeholder write it. `Credentials::renew(&Tokens)` applies an exchange's result. `Tokens`
is `{ access_token, refresh_token, id_token, account_id, expires_at }`.

`Draft` is a tree's rendered request: `{ method, path, query, headers, body }`. `Draft::get`,
`Draft::post`, `Draft::post_json` and `Draft::post_form` build the common shapes.

## Call order

`Endpoint::build_call(draft, credentials, now)` builds the call and sends nothing:

1. Refuse with `Error::Renewal` if `expires_at` has passed, unless the auth is `GitHubToken`:
   the expiry dates the access token, and the GitHub token outlives it.
2. Fill `{field}` placeholders in the base URL and the path from the credentials, percent-encoded:
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

`source.rs` defines `Source` and `SourceHeader`: the static rows that
[account-state](account-state.md#sources), [model-list](model-list.md#protocols) and web search
read from. Each tree's `find_source` is one match over its protocols, so no protocol is left
without its row. `Source::to_endpoint` turns a row into an `Endpoint`: the caller's
`base_url`/`path` override the row's, and a row with neither is `Error::Build`. The draft decides
the method, query and body, so the same row serves an account read's `GET` and a search's `POST`.

- `codex_oauth`: `refresh` POSTs JSON to `https://auth.openai.com/oauth/token` with the Codex CLI
  client id; `post_authorization_code` POSTs a browser login's code as a form to the same endpoint.
  `account_id` comes from the id-token claim, and `expires_at` from the access-token JWT, falling
  back to `expires_in`. Debug builds honor `WISH_TEST_CODEX_ISSUER`.
- `copilot_oauth`: `request_device_code` and `poll_device_code` run GitHub's device flow as forms
  to `https://github.com/login/device/code` and `/login/oauth/access_token` under the OAuth app
  Copilot's editors use; GitHub answers a pending, slowed or refused poll with `200` and an
  `error`. `exchange` GETs `https://api.github.com/copilot_internal/v2/token` with the GitHub token
  and Copilot's editor headers; the session's `token` is the access token and its `expires_at` the
  expiry. Debug builds honor `WISH_TEST_GITHUB` for both hosts.

Every exchange is one attempt, and deciding when to refresh stays with the caller.
