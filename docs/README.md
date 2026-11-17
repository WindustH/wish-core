# Wish Documentation

- **Users**: [User Guides](user/README.md) — installation, configuration, deployment, and the HTTP API.
- **Developers**: [Developer Notes](developer/README.md) — system architecture, protocols, execution engine, and storage.

For the browser and mobile interface, see [Wish Web](https://github.com/WindustH/wish-web).

## Offline manual

All Markdown chapters in this directory are embedded in the `wish` binary at build time. Read them without a running server, configuration file, session shell, source checkout, or network:

```sh
wish docs                           # chapter index
wish docs getting-started           # installation and first session
wish docs configuration             # complete config.json reference
wish docs architecture              # how Wish works internally
wish docs executor                  # execution and compaction
wish docs search proxy              # case-insensitive full-text search
wish docs search shell proxy        # lines containing both words
wish docs user/configuration.md     # paths from the index also work
wish docs --all > wish-manual.md     # export all embedded chapters
wish docs configuration | less      # use your preferred pager
```

`wish docs show <topic>` is equivalent to `wish docs <topic>`. Output is UTF-8 Markdown, with no terminal escape sequences or automatic pager. Search results include source paths and line numbers. Search exits with status `1` when nothing matches; invalid arguments return `2`. The embedded content matches the source used to build the executable, so install a newer binary to update the offline manual. External links and images are references, not bundled web resources.
