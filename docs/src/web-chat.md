# Web Chat

The chat page of the web UI streams replies, shows each tool call the agent
makes, and lets you approve commands, rewind, edit and queue messages.

## Keyboard

| Key | Where | Effect |
|-----|-------|--------|
| Enter | Composer | Send (Shift+Enter for a new line) |
| Esc | While a reply runs | Stop the reply |
| Esc Esc | Idle, empty composer | Open the rewind picker |
| Enter / A / Esc | A command approval is pending | Allow once / always allow / deny |
| `/` | Composer | Slash command menu: Enter runs a command without arguments, Tab inserts it |
| `@` | Composer, session bound to a project | File picker; inserts `@relative/path` |
| Cmd/Ctrl+Shift+O | Anywhere | New chat |
| Shift+Esc | Chat page | Focus the message box |
| Cmd/Ctrl+Shift+S | Chat page | Collapse or expand the sidebar to its icon rail (remembered per browser) |
| Cmd/Ctrl+B | Anywhere on desktop | Collapse or expand the sidebar |
| Alt+Shift+↓ / ↑ | Chat page | Next / previous session, in sidebar order |
| Alt+Shift+C | Chat page | Copy the last reply as Markdown |
| Cmd/Ctrl+K | Anywhere | Command palette |
| Cmd/Ctrl+/ | Anywhere | List these shortcuts |

## Menus

Right-click (or long-press on a phone, or use the **…** button) a session,
a message or a tool card for its actions. On phones menus open as a sheet
from the bottom of the screen.

- **Sessions**: rename, pin to the top (per browser), move to a project,
  fork, share, archive, delete. A deleted session can be restored with
  **Undo** for five seconds.
- **The chat title**: the same session actions plus auto-title, Markdown
  export, MCP tools on/off, and the debug and full-context views.
- **Messages**: copy, edit and resend, fork from here, rewind; on replies,
  regenerate.
- **Tool cards**: copy the command or target, the output, the arguments.

## Layout

The left sidebar holds everything outside the conversation:

- the agent name and connection status, **New chat** and **Search** (the
  command palette, Cmd/Ctrl+K);
- the project filter, a session filter and the Sessions / Cron tabs, then the
  session list;
- at the bottom, your account (its menu has settings, theme, links and
  sign-out), a settings button, the light / system / dark switch and, when a
  password or passkey is set, sign-out.

Collapse the sidebar to an icon rail with its button or Cmd/Ctrl+B; the
choice is remembered per browser. Settings opens as a full page with its
sections listed on the left.

On a phone the top bar has a menu button that opens the sidebar as a drawer,
the agent name and a new-chat button. Settings opens on its section list;
pick one to open it.

## Command approvals

When `tools.exec.approval_mode` asks for approval, the request appears above
the composer of the session that made it, with a countdown to the server's
timeout. Requests waiting in other sessions are counted in the toolbar badge
and marked in the session list.

- **Allow once** approves this request only.
- **Always allow** also remembers the exact command. Remembered commands are
  kept in `exec-approved-commands.json` in the data directory and survive
  restarts.
- **Deny** refuses it; the agent gets an error.

A request answered elsewhere (another tab, a channel `/approve`) or timed out
disappears from every open page.

## Tool calls

Each call is one line: what it did, on what, and a short result (`+12 −3`
for an edit, the match count for a search, the exit code of a failed
command), with the elapsed time while it runs. Click a line to expand it:
edits show an inline diff, reads the file content, searches their matches,
MCP tools their arguments and result. Failed calls open themselves.
Documents, screenshots and maps a tool returns stay visible.

Consecutive calls are folded into one group line, open while they run.
`task_list` calls are summarised as a checklist pinned under the
conversation while a task is still open.

## Rewind, regenerate, edit

Hover a message you sent:

- **Rewind** restores the state before that message: the conversation (the
  message and everything after it is removed and its text goes back to the
  composer), the files the later turns changed (from
  [checkpoints](checkpoints.md)), or both.
- **Edit** forks the session just before the message, switches to the fork
  and sends the edited text there. The original session is unchanged.

On a reply, **Retry → Regenerate** drops the last reply and asks the same
question again. Rewinding is refused while a reply is running.

## Queued messages

Messages sent while a reply runs wait in a tray above the composer and are
sent in order when the reply ends. Each one can be removed, taken back to
the composer to edit, or sent now, which stops the current reply and answers
that message next.

## Rich content

Code blocks have a copy button. Mermaid code fences render as diagrams, and
TeX between `$$…$$`, `\[…\]`, `$…$` or `\(…\)` renders with KaTeX. Both
libraries load only when a reply contains a diagram or a formula.

## Background sessions

The tab title starts with the number of sessions holding replies you have
not read, for example `(2) antre`. When a reply you started in another
session finishes, a toast offers to open it. Channel and cron sessions
count in the title but do not raise a toast.

## Reconnecting

If the connection drops during a reply, the page reconnects and continues
the reply where it is, including the text streamed while it was offline.
