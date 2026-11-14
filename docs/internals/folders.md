# Folders

The list a user browses is arranged in folders, the way a file system arranges files
(`storage::management::folders`). The engine knows nothing of it: folders are the management
index's, beside the session list.

## Records

Every entry names the folder it is in - a parent pointer, as a directory entry does - and none
names what it holds:

| Table | Holds |
| --- | --- |
| `folders` | `id`, `name`, `parent` (none for the root), `pinned`, `created_at`; indexed by `(parent, pinned, name)` |
| `sessions` | the session records, with `folder` and `pinned`; indexed by `(folder, pinned, updated_at)` |

So the costs are a file system's:

- **Listing a folder** (`ManagementStore::conversations`) reads its folders and sessions
  along their indexes and orders only them - folders by name, then the rest by time, pinned
  ones first in each - so it costs what that folder holds, never what the whole list does. Pages
  are chosen by these columns alone, and only the page's records are read.
- **Moving** (`place`) changes one pointer per entry, whatever the entry holds: nothing below a
  moved folder is touched, as nothing is rewritten when a directory is renamed. A folder cannot go
  into itself or into a folder inside it; whether it would is found by climbing the target's
  parents, as a path is resolved.
- **Deleting a folder** hands what it held to its parent - one update per table along its index -
  and never deletes a session.
- **Counting** what a folder holds, for its row, uses two index counts.

Searching by words, a phase or a tag, or asking for the list `flat`, leaves folders aside: every
session that matches, wherever it is, newest first.

## Where new entries go

`POST /api/sessions` takes a `folder`. A fork goes in the folder of the session it copies.
Sessions created with `wish session` inherit the creating session's folder
(`ManagementStore::folder_of`). Sending a message to a peer creates no list or history object.

## The web app

The list shows one folder at a time, as a file manager does: opening a folder replaces the list
with its listing, and the path above it leads back up. It is read in pages and refetched whole on
`list_changed`, which every folder change, move and pin announces. The list remembers the folder it
was showing. What the user makes from the list - a session or a folder - goes in the folder
shown.

## Format

Format 12 adds `folder` and `pinned` to sessions and groups, everything so far in the root
([migration](migration.md)). Format 15 removes the group tables; sessions retain their folders.
