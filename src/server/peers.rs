//! Peers: what a session sees of the other sessions and the groups (`groups`) through
//! `wish session` in its shell (`cli`), a client of the bridge like `wish mcp`. The model is told
//! none of them, only how to look: listing, making, configuring and messaging sessions never
//! changes its request. What a session may do to another is the bridge's rule
//! (`http::peers`): it sees every session and group, messages any session, and configures or
//! deletes only itself and the sessions it made.
pub mod cli;

/// What the model is told about other sessions and groups: how to reach them, and none of them.
pub const INSTRUCTIONS: &str = "# Sessions and groups\n\
   Other sessions are agents like you, each with its own model, memory and shell. Groups are chats \
   among the user and several sessions. The `wish session` command in the shell reaches them; none \
   are listed here.\n\
   - `wish session list` shows the sessions and groups; `wish session show <name>` shows one, a \
   group with its recent messages.\n\
   - `wish session create <name> --instructions <text>` makes a session (with `--model`, \
   `--provider`, `--cwd`, `--no-shell` as needed); `wish session config` and `wish session delete` \
   change or remove one you made, or yourself (a name or model even while running).\n\
   - `wish session send <session|group> <text>` sends a message: to a group you are in, or to a \
   session, in a group of the two of you and the user. It wakes the group's other members.\n\
   - `wish session group create <name> <session>...`, `group add`, `group rename` and `group leave` \
   manage groups.\n\
   A message headed `[Group \"name\" · from X]` was posted in that group. Your answer to it stays \
   here, between you and the user; to say something in the group, send it there. Send when you \
   have something to add, and let it be when you have not.";
