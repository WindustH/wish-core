# Groups and peers

A group is a chat among the user and several sessions, like a chat app's group chat; peers is
what a session sees of the other sessions and the groups, through `wish session` in its shell.
Both are server-owned: the engine knows only messages and notes in a session's history. A session
takes part in a group as a person would: a post wakes it, and it speaks only by sending.

```text
user --POST /groups/{id}/messages--> App::post --+-> group_messages (the transcript)
session --wish session send--> http::peers::send -+-> member queues (User messages)
                                                  +-> schedule the other members
```

## Records (`server::management::groups`)

A group runs nothing, so it is not a session: it lives in `management.sqlite` beside the session
list, in three tables.

| Table | Holds |
| --- | --- |
| `groups` | `id`, `name`, `created_by` (the session that made it with `wish session`), `created_at` and `updated_at` |
| `group_members` | `(group_id, session_id, position)`, `WITHOUT ROWID`; indexed by `session_id` for a session's groups |
| `group_messages` | The transcript: `(group_id, seq)` keyed, `WITHOUT ROWID`, with `at`, the author (`author_kind`, `author_id`, `author_name`), `text` and `attachments` (JSON, the user's only) |

`seq` counts up from 0 in each group; a page reads back from `before` on the primary key, and
search is a substring match over one group's text, which stays small enough to scan.
`ManagementStore::conversations` lists sessions and groups together for the web app's list with
one `UNION ALL` ordered by `updated_at`, so paging does not depend on how the two mix. The files
the user posts to a group are in `blobs/<group id>`, served by the same blob routes as a
session's (`http::content`); deleting the group deletes them and its rows, then shrinks the file
(`ManagementStore::shrink`).

## Posting (`App::post`)

A post by `author` with `text`, and for the user optionally an input with images and files:

1. builds what the members are told: `[Group "name" · from X]\n` and the text in one block, or,
   with attachments, that line as a block of its own and then the input as `input::message` makes
   it from the group's files - images inline, files by their path there. It is built once for
   every member, before anything is written, so an input that cannot be read is turned away;
2. appends the message to the transcript and announces `group_changed`;
3. queues every other member that `User` message with metadata `{"source": "group", "group":
   {"id", "name"}, "from": author}`, touches it so the list brings it up, and schedules it.

Every post wakes every other member, whoever wrote it, and nothing counts or limits what sessions
say to one another.

`App::group_of_two` finds the group of exactly two sessions and the user, or makes one named
`A & B`, which is how `wish session send <session>` reaches a session: the user sees it in the
list like any group. `App::create_group` and `App::update_group` keep each member an existing
session, once; deleting a session takes it out of every group (`App::leave_groups`).

## Speaking (`http::peers::send`)

Nothing a session answers goes to a group by itself: its answer to a group's message is its own
conversation's, as a person's thoughts are. It speaks in a group by sending - `wish session send`,
one shell command like any other - which posts as the session (`App::post`) and notes it in its
own history as `SessionEvent::Application` `{"type": "group_message_sent", "group", "text",
"woken"}` through `SessionSender::record_application_event`, so its page shows what it said
where. A session without a shell reads its groups and never speaks there. What a session was
told is in its history as the queued `User` messages. Nothing of a group is in the history search
index: the transcript is searched where it is kept.

## `wish session` (`server::peers`)

`peers::INSTRUCTIONS` is appended to the agent instructions after the skills' when the session has
a shell, and names no session or group; it tells the model that a message headed by a group came
from there, and that it speaks there by sending. `peers::cli` is a bridge client like `wish mcp`'s:

| Command | Route | Rule |
| --- | --- | --- |
| `list`, `show` | `GET .../peers[/{target}]` | Every session and group; a group with its last 20 messages |
| `create` | `POST .../peers` | On the configured defaults, in the maker's directory, with the given instructions (a `System` message and `metadata.agent_custom`, as the web app does); `created_by` is the maker |
| `config`, `delete` | `PATCH`, `DELETE .../peers/{target}` | Only itself (config) or a session it made (`may_manage`); never the user's. Changes go through `manage::update`, as the user's do: a name at any time, a model staged while it runs, instructions only while idle |
| `send` | `POST .../peers/{target}/send` | To a group it is in, or to a session through their group of two, which wakes that session |
| `group create/add/rename/leave` | `POST .../peers/groups`, `.../members`, `PATCH .../peers/{group}`, `.../leave` | A member may add and rename; anyone may leave |

Targets are resolved by id, then by a name only one bears (`resolve`). `http::peers::me` checks
the session's token (`http::bridge`) and its `tools.sessions` switch, which, like the MCP and
skills switches, changes nothing the model is sent. The bridge maps a refusal to exit status 1,
so the model reads why in the command's output.

## Format

Format 9 adds `tools.sessions`, on, to every session record and the defaults; format 10 creates
the group tables and moves the groups earlier builds kept as sessions into them; format 11 drops
the relay limit: the `groups` setting, the groups' relay counts and Wish's notes
([migration](migration.md)).
