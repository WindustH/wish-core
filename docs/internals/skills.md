# Skills

A skill is a directory holding a `SKILL.md`: YAML front matter with a `name` and a `description`,
then instructions, which may point to scripts or reference files beside them. The format is the
one other agents read, so one set of skills can serve them all. Wish tells the model none of them.
The agent instructions of a session with a shell only say how to find them; the model runs
`wish skill` in its shell, through the same bridge as `wish mcp` ([MCP](mcp.md)). Adding, removing
or switching off a skill therefore never changes a byte of the request a conversation's prompt
cache is keyed on, and neither does a session's `tools.skills` switch, which only the bridge
checks.

```text
model --shell_start("wish skill find deploy")--> session shell (WISH_URL, WISH_SESSION, WISH_SESSION_TOKEN)
                                                        |
                                  wish skill  (server::skills::cli, no server started)
                                                        | HTTP on loopback, session token
                                                        v
                         /api/sessions/{id}/skills[/{name}]  (server::http::skills)
                                                        |
                                    server::skills: discover, find, show
```

## Finding skills (`server::skills`)

`roots` gives the places to look, first first: `.agents/skills` under the session's working
directory, Wish's own directory (`App::skills_dir`, `skills` beside the configuration file, made at
startup) and each of `skills.dirs`, with a leading `~` read as the home directory. `discover` walks
each root up to four folders deep: a directory holding a `SKILL.md` is a skill and is not searched
further, hidden directories are skipped, and links are followed with every directory read once. A
skill's `category` is the folders between its root and it, so `configuration/nvim` is `nvim` in
`configuration`.

The front matter is read for what skills use - `key: value` lines, quoted or not, continued on
indented lines, and `|` or `>` blocks - not as full YAML. A skill without a `name` is named after
its directory, and one without a `description` is known by the first line of its instructions.
`Found::usable` keeps the first skill of each name and drops the names in `skills.disabled`;
settings list everything, marking those hidden by an earlier one (`shadowed`) and those switched
off.

`find` ranks skills by a query's words: a word equal to the name counts most, then a word found in
the name, in the category or description, and in the first 64 KiB of the file. A word in a script
written without spaces (Chinese, Japanese, Korean) that is not found whole is matched two
characters at a time. `show` reads the instructions without the front matter and lists the skill's
other files, relative to its directory, up to 60 of them.

Discovery reads the disk on each request, in `blocking`: skills and the configuration's
`skills` section take effect on the next command, with no restart and no cache to empty.

## `wish skill` (`server::skills::cli`)

Like `wish mcp`, it runs before any configuration is read, reads `WISH_URL`, `WISH_SESSION` and
`WISH_SESSION_TOKEN` through `server::bridge::client::BridgeClient`, and prints what the model
reads: `name - description` lines (by folder for `list`), and for `show` the directory and files
before the instructions. Its exit status is `0` when done, `1` when no skill matched or has the
name asked for, and `2` when nothing was asked.

## The bridge routes (`server::http::skills`)

`GET /api/sessions/{id}/skills` lists the skills the session can use, or with `?query=` those that
fit, best first; `GET /api/sessions/{id}/skills/{name}` shows one, matching the name without
regard to case when no exact match exists. Both take the session's own token
(`server::http::bridge`) and answer `409` with a sentence meant for the model while the session
has `tools.skills` off. `GET /api/skills` and `GET /api/skills/{name}` serve the settings page with
the application's token, from Wish's own directory and the configured ones only.
