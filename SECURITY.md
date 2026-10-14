# Security policy

## Reporting a vulnerability

Please report security problems privately, not in a public issue. Use GitHub's
private vulnerability reporting: open the repository's **Security** tab and
choose **Report a vulnerability**. Include the version or commit, what you did,
and what you expected to happen. We will acknowledge the report and keep you
updated as it is fixed.

## Threat model

Tessel is a local desktop app for one user on Linux. It has no server and
makes no network connections of its own. What it protects against:

### The agent link is local only

The built-in MCP server (for agents such as Claude Code) listens only on a unix
socket, never on a TCP port.

- Each window binds `$XDG_RUNTIME_DIR/tessel/<pid>.sock` with mode `0600`, in a
  directory with mode `0700`. The app creates the directory or tightens one it
  already owns.
- The app and the `tessel mcp` bridge both check the directory before trusting
  it: it must not be a symlink, must be owned by your user, and must have no
  group or other permissions. Otherwise they refuse to bind or connect, so
  another local user cannot plant a fake window.
- The agent speaks to the app only through this link, so anyone who can reach
  the socket is you (or a process running as you).

### `export_file` limits where an agent can write

An agent's `export_file` call is checked twice, by the agent and again by the
app right before writing (`check_export_path` in
`crates/agent/src/handle.rs`):

- the path must be absolute and end in `.png` or `.svg` to match the format;
- it must be inside the document's folder or your home folder, after resolving
  `..` and symlinks in the folders that exist (`..` through a folder that does
  not exist yet is refused);
- no folder or file below that root may start with `.`, so hidden folders and
  dotfiles such as `~/.ssh` or `~/.bashrc` are out of reach;
- the file itself is never a symlink, and an existing file is only replaced if
  it is a regular file;
- the write goes to a new temporary file that is renamed over the target, so it
  never follows a symlink or hard link.

### Untrusted documents

A `.tessel.json` file you download is untrusted input. Tessel treats it so:

- Asset (image) reads are limited to regular files (no symlinks, FIFOs or
  directories) of at most 64 MiB; anything else is skipped with a warning.
- Component expansion has a budget: one instance may expand to at most 100,000
  nodes and 32 levels of nesting. Over that, it stays unexpanded and is
  reported as a problem. This stops a small file from exhausting memory.
- A document with problems (validation errors) opens read-only, with a banner,
  and agents cannot edit it either.
- Strings from the document that reach exported code (font families, asset
  names, path data, comments, titles) are whitelisted or escaped so they cannot
  break out of strings, comments or markup in the HTML, CSS, Tailwind or React
  output. This is covered by the hostile-string tests in
  `crates/export/tests/hostile.rs`. Still review generated code before you
  run it.

## Prompt injection through documents

**Layer text and names from a document reach a connected agent.** When an
agent reads the document (`get_document`, `get_node`, `find_nodes`, ...), it
sees every layer's name and text. A document you downloaded can hold text
written to look like instructions ("ignore previous instructions and ..."),
and an agent that follows it could misuse its other tools or access. Tessel
cannot detect this.

When you open a file you do not trust:

- **Untick "Agents may edit"** in the status bar. The document becomes
  read-only to agents (they can still read, select and take screenshots), but
  this does not stop them reading the text, so
- do not connect an agent that has access to things you care about (shell,
  files, credentials) while you work on an untrusted file, or disconnect it
  from the status bar.
