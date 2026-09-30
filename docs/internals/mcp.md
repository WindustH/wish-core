# MCP

Wish calls MCP servers on the model's behalf without ever showing their tools to the model as tools.
Every session with a shell gets one fixed section in its agent instructions, the system message
Wish puts first in every request (`server::mcp::INSTRUCTIONS`, added by
`server::session_model::agent_instructions` when the request has `shell_start`), and a few variables in its
shell's environment. The model finds servers
and tools by running `wish mcp list` and `wish mcp describe`, calls them with `wish mcp call`, and
reads the answers as command output. What servers exist, what tools they offer and whether the
session may use them can all change at any time without changing a byte of the request a
conversation's prompt cache is keyed on: the session's `tools.mcp` switch is enforced only by the
bridge, which answers a session with MCP off by saying so, and the model reads that like any other
output.

```text
model --shell_start("wish mcp call s/t '{...}'")--> session shell (WISH_URL, WISH_SESSION, WISH_MCP_TOKEN)
                                                        |
                                    wish mcp  (server::mcp::cli, no server started)
                                                        | HTTP on loopback, session token
                                                        v
                          server::http::mcp  /api/sessions/{id}/mcp/{servers,tool,call}
                                                        |
                                               server::mcp::McpHub
                                                        |
                                       mcp::Connection (rmcp) -- stdio process / Streamable HTTP
```

## `mcp`: the client

`src/mcp.rs` is the engine side and knows nothing of the server. The protocol is rmcp's (the
official Rust SDK, with its default features off: `client`, `transport-child-process` and
`transport-streamable-http-client` only). What wish adds is how a server is reached and the narrow
surface the application uses:

| Item | Meaning |
| --- | --- |
| `Endpoint::Stdio { command, args, env, cwd }` | A local program. Started through `process-wrap` in a process group of its own (a job object on Windows) with kill-on-drop, so closing it ends `npx` and the server behind it. Its standard error is read into an 8 KiB tail |
| `Endpoint::Http { url, headers, proxy }` | A remote server over Streamable HTTP |
| `Connection::open` | Starts or reaches the server and runs the legacy `initialize` handshake, within 120 seconds (a first `npx` run downloads the server). A failure carries the end of standard error |
| `list_tools`, `call_tool` | Tools as JSON values, as the protocol describes them. A call is sent as a cancellable request with `timeout` reset by progress reports; a call dropped before its answer sends `notifications/cancelled` |
| `McpError` | `Connect` (could not start or reach), `Rejected` (an error answer), `Unknown` (the connection closed or the time ran out: a call may have taken effect) |

`mcp::http::HttpClient` is rmcp's `StreamableHttpClient` over wish's own reqwest client, built with
the same `transport::apply_proxy` as provider calls, so rmcp's reqwest (another major version)
never enters the build. rmcp keeps the session (`Mcp-Session-Id`, re-initializing after a `404`);
the client sends one request at a time and reads the reply into a JSON-RPC message, an SSE stream
or an acknowledgement. It is more lenient than rmcp's own in one place: a successful reply to a
notification that carries no JSON-RPC message is an acknowledgement whatever its headers, because
some servers (Zhipu's) answer notifications with a bare `200` that has neither a length nor a
content type. A `server/discover` rejected by a server that predates it is answered with the
request's id, which is what sends rmcp's handshake back to `initialize`.

## `server::mcp`: instances

`McpHub` holds the configuration, the live instances and a catalog of tools per server.

- An instance is keyed by server and, for `scope: "session"` (the default), by session. It is
  created on first use, resolved to an `Endpoint` then (the session's working directory, the
  `auth_provider`'s current key, the proxy policy), and connected lazily through a `OnceCell`, so
  concurrent first calls share one start.
- The catalog is shared by all instances of a server: the same configuration offers the same tools.
  Listing and describing read it and connect only when it is empty. A connection fills it when it
  opens; `notifications/tools/list_changed` empties it so the next listing reads again.
- A call marks its instance busy for its duration and stamps `last_used` when it ends. The reaper
  task (every 5 seconds, in `App`'s task tracker) closes instances past their server's
  `idle_timeout` with no call running, and instances whose connection has closed; the next call
  starts a new one.
- A configuration save (`apply`) closes every instance whose server changed, was removed or
  disabled, or would now resolve to a different endpoint (a rotated key, another proxy), and drops
  the catalogs of changed servers. Deleting a session or switching its MCP off closes its own
  instances. Shutdown closes all of them after the task tracker drains.
- Closing an instance waits for calls still holding it: the connection goes when the last
  reference does.

`store_binary_content` replaces the base64 of image, audio and resource-blob content in a call's
result with a file under `shell/<session>/mcp/`, named by digest with an extension from the MIME
type, before the result crosses the bridge.

## The bridge and `wish mcp`

`SessionSlot::open` makes the session's `mcp_token` (two random UUIDs) and adds `WISH_URL`,
`WISH_SESSION`, `WISH_MCP_TOKEN` and a `PATH` led by `data_dir/bin` to the shell's environment
(`server::mcp::bridge`),
whether or not MCP is on; the bridge checks the switch on every request and refuses with `409` and
a sentence meant for the model: MCP is disabled here, and the user can enable it in the session's
settings. `data_dir/bin/wish` is a
link to the running program, made at startup, so the command in the shell is always this build.
`WISH_URL` is set once the listener is bound, with an unspecified address replaced by loopback.

The bridge routes sit outside the application's `authorize` layer and check the session's token in
the handler: a request is served only for the session whose token it carries, and only while that
session is open, since a shell holding a token belongs to an open session. A failure carries
`details.kind` (`not_found`, `connect`, `rejected`, `unknown`), which `wish mcp` turns into its
exit status (`2`, `2`, `2`, `3`; a result with `isError` exits `1`). The call handler's future is
dropped when the command's connection closes, which drops the pending call and sends the
cancellation.

`wish mcp` (`server::mcp::cli`) runs before any configuration is read: `server::run` hands it the
arguments after `mcp`. It reaches the bridge with proxies disabled, since a proxy from the shell's
environment must not stand between it and the loopback address.
