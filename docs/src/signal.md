# Signal

Moltis can receive and send Signal messages through an external
[`signal-cli`](https://github.com/AsamK/signal-cli) daemon. The Moltis process
talks to the daemon over local HTTP JSON-RPC for outbound messages and Server
Sent Events for inbound messages.

## How It Works

```
Signal Network
      │
      ▼
signal-cli daemon  ── HTTP JSON-RPC + SSE ──  moltis-signal
      │                                      │
      └──────── linked Signal account ──────┴── Moltis Gateway
```

Moltis does not embed libsignal directly. The Signal account, device link, and
Signal protocol state stay inside signal-cli, which keeps the Moltis integration
smaller and avoids coupling releases to Signal's native protocol internals.

## Prerequisites

Install and configure signal-cli first:

```bash
signal-cli -u +15551234567 register
signal-cli -u +15551234567 verify 123456
signal-cli --account +15551234567 daemon --http 127.0.0.1:8080
```

You can also use signal-cli's linked-device flow instead of registering a new
number. Keep the HTTP daemon reachable only from trusted local services.

## Configuration

Add a `[channels.signal.<account-id>]` section to `moltis.toml`:

```toml
[channels.signal.personal]
account = "+15551234567"
http_url = "http://127.0.0.1:8080"
dm_policy = "allowlist"
allowlist = ["+15557654321", "550e8400-e29b-41d4-a716-446655440000"]
group_policy = "disabled"
mention_mode = "mention"
otp_self_approval = true
otp_cooldown_secs = 300
text_chunk_limit = 4000
```

Make sure `"signal"` is included in `channels.offered` if you customize that
list. It is included by default.

## Fields

| Field | Required | Default | Description |
|-------|----------|---------|-------------|
| `account` | no | account ID | Signal account loaded in signal-cli, usually an E.164 phone number |
| `account_uuid` | no | not set | Optional Signal account UUID for allowlist matching |
| `http_url` | no | `http://127.0.0.1:8080` | signal-cli daemon HTTP URL |
| `dm_policy` | no | `allowlist` | `open`, `allowlist`, or `disabled` |
| `allowlist` | no | `[]` | Allowed sender phone numbers, UUIDs, or normalized identifiers |
| `group_policy` | no | `disabled` | `open`, `allowlist`, or `disabled` |
| `group_allowlist` | no | `[]` | Allowed Signal group IDs |
| `mention_mode` | no | `mention` | `mention`, `always`, or `none` |
| `ignore_stories` | no | `true` | Ignore story events from signal-cli |
| `otp_self_approval` | no | `true` | Let unknown DM senders self-approve with a PIN challenge |
| `otp_cooldown_secs` | no | `300` | Cooldown after 3 failed OTP attempts |
| `text_chunk_limit` | no | `4000` | Maximum UTF-8 bytes per outbound text chunk |

## Voice Notes

Voice notes from approved senders are transcribed with the speech-to-text
provider configured under **Settings -> Voice** (`[voice.stt]`), then sent to
the agent as the user message. Any text sent with the note is kept as a
caption above the transcript. The audio is also saved to the session media so
it can be played back from the web UI.

Moltis fetches the audio from the daemon with signal-cli's `getAttachment`
JSON-RPC method, so the daemon does not need to share a filesystem with
Moltis. Audio is recognized from the attachment content type (AAC/M4A, Ogg
Opus, MP3, WAV, WebM), falling back to the file extension.

The sender always gets an answer:

- No STT provider configured: a reply pointing to **Settings -> Voice**.
- Voice note larger than 25 MB, download failure, transcription failure, or
  empty transcript: a short reply asking to try again or type the message.
- When a caption was sent with the note, the caption is forwarded to the agent
  instead of the error reply.

In groups, the usual `group_policy` and `mention_mode` rules apply before any
download. A voice note cannot carry a mention, so with `mention_mode =
"mention"` it is ignored unless it has a caption that mentions the account.

## Images and Documents

Other attachments from approved senders are downloaded the same way and handed
to the agent with the message text:

- **Images** (JPEG, PNG, GIF, WebP, up to 20 MB) are resized for the model and
  sent as image input, so a vision-capable model can see them.
- **Documents** (any other type, up to 25 MB) are saved to the session media
  directory and the agent receives their name, type and local path. Text-like
  files (plain text, Markdown, CSV, JSON, XML, YAML, TOML, HTML) and PDFs also
  have their text inlined in the message, truncated to about 24,000
  characters.

A message can carry several attachments, including a voice note. An attachment
that is too large, cannot be downloaded, or cannot be saved is never dropped
silently: the agent receives a note such as `[Document - download failed]` and
can tell the sender.

## Current Limits

Outbound media uses the text fallback: replies are sent as text only.
