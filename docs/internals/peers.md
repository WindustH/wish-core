# Session peers

`application/session_access` exposes the direct tool and authenticated shell bridge.
`peer_commands` resolves session IDs or unambiguous names and enforces sender capability
and configure/delete ownership checks.

`send` writes one `User` message to the recipient's durable session queue, with
`source: session_message` and the sender's ID/name in metadata and a text heading.
It schedules the recipient immediately after enqueue. A failed optional index touch does
not change successful delivery into a retryable error. The caller's ordinary tool result
records the target and queue entry; there is no extra communication history object.
Normal replies are not relayed. A recipient explicitly calls `send` to reply.

Format migration 15 drops groups, memberships and group messages. It records retired blob
owners transactionally and removes their directories after commit; interrupted cleanup
resumes on startup. Session-owned image copies and existing session histories are preserved.
