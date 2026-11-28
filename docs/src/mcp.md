# MCP Servers

Antre supports the [Model Context Protocol (MCP)](https://modelcontextprotocol.io) for connecting to external tool servers. MCP servers extend your agent's capabilities without modifying Antre itself.

## What is MCP?

MCP is an open protocol that lets AI assistants connect to external tools and data sources. Think of MCP servers as plugins that provide:

- **Tools** — Functions the agent can call (e.g., search, file operations, API calls)
- **Resources** — Data the agent can read (e.g., files, database records)
- **Prompts** — Pre-defined prompt templates

## Supported Transports

| Transport | Description | Use Case |
|-----------|-------------|----------|
| **stdio** | Local process via stdin/stdout | npm packages, local scripts |
| **Streamable HTTP** | Remote server via HTTP | Cloud services, shared servers |

## Adding an MCP Server

### Via Web UI

1. Go to **Settings** → **MCP Servers**
2. Click **Add Server**
3. For remote Streamable HTTP servers, enter the server URL and any optional request headers
4. Click **Save**

After saving a remote server, Antre only shows a sanitized URL plus header names/count in the UI and status views. Stored header values stay hidden.

### Via Configuration

Add servers to `antre.toml`:

```toml
[mcp]
request_timeout_secs = 30

[mcp.servers.filesystem]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "/Users/me/projects"]

[mcp.servers.github]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-github"]
env = { GITHUB_TOKEN = "ghp_..." }
request_timeout_secs = 90

[mcp.servers.remote_api]
transport = "sse"
url = "https://mcp.example.com/mcp?api_key=$REMOTE_MCP_KEY"
headers = { Authorization = "Bearer ${REMOTE_MCP_TOKEN}" }

[mcp.servers.remote_http]
transport = "streamable-http"
url = "https://mcp.example.com/mcp"
headers = { Authorization = "Bearer ${API_KEY}" }
```

Remote URLs and headers support `$NAME` and `${NAME}` placeholders. For live remote servers, placeholder values resolve from Antre-managed env overrides, either `[env]` in config or **Settings** → **Environment Variables**.

## Popular MCP Servers

### Official Servers

| Server | Description | Install |
|--------|-------------|---------|
| **filesystem** | Read/write local files | `npx @modelcontextprotocol/server-filesystem` |
| **github** | GitHub API access | `npx @modelcontextprotocol/server-github` |
| **postgres** | PostgreSQL queries | `npx @modelcontextprotocol/server-postgres` |
| **sqlite** | SQLite database | `npx @modelcontextprotocol/server-sqlite` |
| **puppeteer** | Browser automation | `npx @modelcontextprotocol/server-puppeteer` |
| **brave-search** | Web search | `npx @modelcontextprotocol/server-brave-search` |

### Community Servers

Explore more at [mcp.so](https://mcp.so) and [GitHub MCP Servers](https://github.com/modelcontextprotocol/servers).

## Configuration Options

```toml
[mcp]
request_timeout_secs = 30       # Global default timeout for MCP requests

[mcp.servers.my_server]
command = "node"                # Required for stdio transport
args = ["server.js"]            # Optional arguments

# Optional environment variables
env = { API_KEY = "secret", DEBUG = "true" }

# Optional: per-server timeout override
request_timeout_secs = 90

# Optional: remote transport
transport = "sse"               # "stdio" (default), "sse", or "streamable-http"
url = "https://mcp.example.com/mcp"  # Required when transport = "sse" or "streamable-http"
headers = { "x-api-key" = "$REMOTE_MCP_KEY" }  # Optional request headers
```

## Request Timeouts

Antre applies MCP request timeouts in two layers:

- `mcp.request_timeout_secs` sets the global default for every MCP server
- `mcp.servers.<name>.request_timeout_secs` optionally overrides that default for a specific server

This is useful when most local MCP servers respond quickly, but one remote server or one expensive tool server needs a longer timeout.

```toml
[mcp]
request_timeout_secs = 30

[mcp.servers.remote_api]
transport = "sse"
url = "https://mcp.example.com/mcp"
request_timeout_secs = 120
```

In the web UI, the MCP settings page lets you edit both the global default timeout and the optional timeout override for each configured server.

## Remote Server Secrets and Placeholders

Remote MCP servers (SSE or Streamable HTTP) often expect API keys or bearer tokens in the URL query string or request headers. Antre supports both patterns.

```toml
[mcp.servers.linear_remote]
transport = "sse"
url = "https://mcp.example.com/mcp?api_key=$REMOTE_MCP_KEY"
headers = {
  Authorization = "Bearer ${REMOTE_MCP_TOKEN}",
  "x-workspace" = "team-a",
}
```

- Use `$NAME` or `${NAME}` placeholders in remote `url` and `headers`
- Placeholder values resolve from Antre-managed env overrides, either `[env]` in config or **Settings** → **Environment Variables**
- UI and API status payloads only expose sanitized URLs plus header names/count, not raw header values
- Query-string secrets are redacted when Antre displays a remote URL after save

## Server Lifecycle

```
┌─────────────────────────────────────────────────────┐
│                   MCP Server                         │
│                                                      │
│  Start → Initialize → Ready → [Tool Calls] → Stop   │
│            │                       │                 │
│            ▼                       ▼                 │
│     Health Check ◄─────────── Heartbeat             │
│            │                       │                 │
│            ▼                       ▼                 │
│    Crash Detected ───────────► Restart              │
│                                    │                 │
│                              Backoff Wait            │
└─────────────────────────────────────────────────────┘
```

### Health Monitoring

Antre monitors MCP servers and automatically:

- Detects crashes via process exit
- Restarts with exponential backoff
- Disables after max restart attempts
- Re-enables after cooldown period

## Using MCP Tools

Once connected, MCP tools appear alongside built-in tools. The agent can use them naturally:

```
User: Search GitHub for Rust async runtime projects

Agent: I'll search GitHub for you.
[Calling github.search_repositories with query="rust async runtime"]

Found 15 repositories:
1. tokio-rs/tokio - A runtime for writing reliable async applications
2. async-std/async-std - Async version of the Rust standard library
...
```

## Beyond Tools: Resources, Prompts, Roots, Sampling and Elicitation

Antre implements the MCP 2025-11-25 revision and accepts servers that
negotiate an older one (2025-06-18, 2025-03-26, 2024-11-05).

### Resources

Servers can expose resources (files, records, documents) and URI templates.

- The agent gets two tools: `mcp_resources_list` (optionally for one server)
  and `mcp_resource_read` (`server` + `uri`). Text is returned up to 100 KB per
  item; binary content is described, not returned.
- The UI uses the RPC methods `mcp.resources.list`, `mcp.resources.read`,
  `mcp.resources.subscribe` and `mcp.resources.unsubscribe`.
- Subscribed resources emit `mcp.resource.updated` events
  (`{ server, uri }`); list changes emit `mcp.resources.changed`.

### Prompts

Server prompts are offered as slash-command candidates named
`<server>:<prompt>`.

- `mcp.prompts.list` returns `{ commands: [{ command, server, prompt, title,
  description, arguments }] }` for the composer.
- `mcp.prompts.get` (`server`, `name`, `arguments`) renders a prompt; missing
  required arguments are reported before the server is called.

### Roots

Servers that ask for `roots/list` see the agent workspace (the data
directory) and every project directory as `file://` roots. Untrusted servers
see an empty list. Running servers are told when the list changes.

### Elicitation

A server can ask the user a structured question while a tool runs
(`elicitation/create`, form mode).

1. The gateway broadcasts `mcp.elicitation.requested` with
   `{ id, server, sessionKey, message, schema, timeoutSecs }`.
2. If the session belongs to a channel (Telegram, Discord…), the question and
   its fields are also sent there as text, with a pointer to the web UI.
3. The UI answers with `mcp.elicitation.resolve`
   (`requestId`, `action`: `accept` | `decline` | `cancel`, `content`).
   Accepted answers are checked against the schema; an invalid answer is
   rejected and the request stays open.
4. Without an answer within `mcp.elicitation_timeout_secs` (300 s) the request
   is cancelled and the channel is told. `mcp.elicitation.resolved` is
   broadcast in every case.

`mcp.elicitation.list` returns the pending requests. Both methods need the
`operator.approvals` scope. URL-mode elicitation is not supported yet.

### Sampling

A server can ask the model for a completion (`sampling/createMessage`). It
is off by default and enabled per server:

```toml
[mcp.servers.notes.sampling]
enabled = true
max_tokens_per_request = 1024
max_requests_per_session = 20
max_tokens_per_session = 50000
# model = "anthropic/claude-haiku-4-5"
```

The request is answered with the model of the chat session whose tool call
triggered it (or `model`, or the first configured provider). Output stops at
the request's token cap. Counters are per chat session and kept in memory.
Each completed request broadcasts `mcp.sampling.completed`.

### Trust tiers

| | `trusted` (default) | `ask` | `untrusted` |
|---|---|---|---|
| Tool calls | run directly | user approves each call | user approves each call |
| Tool results | as returned | as returned | wrapped as untrusted data |
| Sampling | if enabled | if enabled, approved per request | never |
| Elicitation | yes | yes | declined |
| Roots | workspace + projects | workspace + projects | none |

```toml
[mcp.servers.web-scraper]
trust = "untrusted"
```

Approvals go through the same flow as shell command approvals: the web UI
shows them, and channel users get `/approve N` / `/deny N`. With no approval
flow available a call that needs approval is refused.

### Many servers: deferred tool schemas

With `tools.registry_mode = "auto"`, MCP tools are moved behind
`tool_search` once there are more than `tools.lazy_mcp_threshold` of them, so
connecting servers does not grow the prompt. See
[Tool Registry](tool-registry.md).

### MCP Apps

Protocol support only: tools that declare `_meta.ui.resourceUri` are listed
with that metadata by `mcp.tools`, tools marked app-only are hidden from the
model, and `mcp.apps.resource` (`server`, `uri`) returns a validated
`ui://` document (`html`, `mimeType`, `uiMeta` with CSP and permissions).
Set `mcp.apps_enabled = true` to advertise the extension; the web UI does not
render apps yet.

### Transports

Server-to-client requests work over stdio and Streamable HTTP. Over
Streamable HTTP, requests sent inside a response stream are answered with a
separate POST, and the optional GET stream is opened for notifications sent
outside requests. The legacy SSE transport cannot receive server requests, so
sampling, elicitation and roots are not advertised to those servers.

## Creating an MCP Server

### Simple Node.js Server

```javascript
// server.js
import { Server } from "@modelcontextprotocol/sdk/server/index.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";

const server = new Server(
  { name: "my-server", version: "1.0.0" },
  { capabilities: { tools: {} } }
);

server.setRequestHandler("tools/list", async () => ({
  tools: [{
    name: "hello",
    description: "Says hello",
    inputSchema: {
      type: "object",
      properties: {
        name: { type: "string", description: "Name to greet" }
      },
      required: ["name"]
    }
  }]
}));

server.setRequestHandler("tools/call", async (request) => {
  if (request.params.name === "hello") {
    const name = request.params.arguments.name;
    return { content: [{ type: "text", text: `Hello, ${name}!` }] };
  }
});

const transport = new StdioServerTransport();
await server.connect(transport);
```

### Configure in Antre

```toml
[mcp.servers.my_server]
command = "node"
args = ["server.js"]
```

## Debugging

### Check Server Status

In the web UI, go to **Settings** → **MCP Servers** to see:

- Connection status (connected/disconnected/error)
- Available tools
- Sanitized remote URL and configured header names
- Recent errors

### View Logs

MCP server stderr is captured in Antre logs:

```bash
# View gateway logs
tail -f ~/.antre/logs.jsonl | grep -i mcp
```

### Test Locally

Run the server directly to debug:

```bash
echo '{"jsonrpc":"2.0","method":"tools/list","id":1}' | node server.js
```

## OAuth Authentication

Remote MCP servers can require OAuth 2.1 authentication. Antre handles this automatically — when a server returns `401 Unauthorized`, the OAuth flow starts without any manual configuration.

### How It Works

1. Antre connects to the remote MCP server
2. The server returns `401 Unauthorized` with a `WWW-Authenticate` header
3. Antre discovers the authorization server via [RFC 9728](https://www.rfc-editor.org/rfc/rfc9728) (Protected Resource Metadata)
4. Antre performs [dynamic client registration](https://www.rfc-editor.org/rfc/rfc7591) (RFC 7591)
5. A PKCE authorization code flow opens your browser for login
6. After login, tokens are stored and used for all subsequent requests

Client registrations and tokens are cached locally, so you only need to log in once per server.

### Manual OAuth Configuration

If a server doesn't support standard OAuth discovery, you can configure credentials manually:

```toml
[mcp.servers.private_api]
url = "https://mcp.example.com/mcp"
transport = "sse"

[mcp.servers.private_api.oauth]
client_id = "your-client-id"
# Optional: required by some servers for token exchange even with PKCE.
client_secret = "your-client-secret"
auth_url = "https://auth.example.com/authorize"
token_url = "https://auth.example.com/token"
scopes = ["mcp:read", "mcp:write"]
```

### Re-authentication

If your session expires or tokens are revoked, Antre automatically re-authenticates on the next `401` response. You can also trigger re-authentication manually via the `mcp.reauth` RPC method.

## Running MCP Servers in Docker

When running Antre in Docker, you have two options for stdio-based MCP servers:

### Using the built-in Node.js

The Antre Docker image includes Node.js and npm, so most MCP servers work out of the box:

```toml
[mcp.servers.filesystem]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "/data"]
```

> **Tip:** Mount `/home/antre/.npm` as a named volume so packages are only
> downloaded once, and bind-mount any directories the MCP server needs to
> access:
>
> ```sh
> docker run \
>   -v antre-npm-cache:/home/antre/.npm \
>   -v /host/path/to/data:/data \
>   ...
> ```

### Using Docker containers

Since the Antre image ships the Docker CLI (`docker-ce-cli`), and given a Docker daemon is reachable via the mounted socket, you can also run MCP servers as isolated containers. This is useful when you need a specific Node version, want stronger isolation, or prefer official MCP Docker images:

```toml
# Run an npm-based MCP server in a container
[mcp.servers.filesystem]
command = "docker"
args = [
  "run", "--rm", "-i",
  # NOTE: bind-mount paths resolve against the HOST filesystem, not the
  # Antre container. Use the same host path you mounted into Antre.
  "-v", "/data:/data",
  # Cache npm downloads across container restarts
  "-v", "antre-npx-cache:/root/.npm",
  "--entrypoint", "npx",
  "node:22-alpine",
  "-y", "@modelcontextprotocol/server-filesystem", "/data",
]

# Use an official MCP Docker image
[mcp.servers.memory]
command = "docker"
args = ["run", "--rm", "-i", "mcp/memory"]
```

The named volume `antre-npx-cache` persists the npm cache across container restarts, avoiding re-downloads on every MCP server restart. For air-gapped environments, consider pre-building a custom image with the MCP package installed.

When using containerized MCP servers, remember to mount any directories the server needs access to with `-v`. Because Antre talks to the Docker daemon via the mounted socket, bind-mount paths (`-v`) always reference the **host** filesystem — not the Antre container's filesystem.

## Security Considerations

```admonish warning
MCP servers run with the same permissions as Antre. Only use servers from trusted sources.
```

- **Review server code** before running
- **Limit file access** — use specific paths, not `/`
- **Use environment variables** for secrets
- **Prefer placeholders** in remote URLs and headers (`$NAME` / `${NAME}`) instead of hardcoding secrets repeatedly
- **Network isolation** — run untrusted servers in containers
- **Set a trust tier** — `trust = "ask"` or `"untrusted"` for servers you do not control

## Troubleshooting

### Server won't start

- Check the command exists: `which npx`
- Verify the package: `npx @modelcontextprotocol/server-filesystem --help`
- Check for port conflicts

### Tools not appearing

- Server may still be initializing (wait a few seconds)
- Check server logs for errors
- Verify the server implements `tools/list`

### Server keeps restarting

- Check stderr for crash messages
- Increase `max_restart_attempts` for debugging
- Verify environment variables are set correctly
