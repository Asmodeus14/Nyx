# The terminal browser

`apps/terminal` is Nyx's web browser. There is no graphical one — it was deleted deliberately
(commit `f0ec92e`) and browsing moved to commands over the same `libs/net` transport.

## Commands

| | |
|---|---|
| `get <url>` | fetch and render. A bare host gets **https**, not http |
| `links` | list this page's links, numbered as they appeared |
| `open <n>` | follow link *n* · `open <url>` goes straight to a URL |
| `back` / `b`, `forward` / `f` | move through this session's history |
| `reload` / `r`, `history` | refetch · list everywhere this session went |
| `find <text>`, `next` / `n` | search **the page**, not the scrollback |
| `reader` | drop nav/sidebar/footer chrome; re-renders with no refetch |
| `page`, `stop` | re-print the loaded page · abandon a load |
| `dns <host>` \| `cache` \| `flush` | resolve with timing · inspect · clear |
| `fetch <url>` | raw diagnostic: status, size, first 1 KB unrendered. **Blocks** |
| ↑ / ↓ | command history |

`get` is non-blocking. `fetch` is the raw diagnostic and freezes its window for the duration — that
is deliberate, and it is why `get` exists.

## The flow

```
URL text
  → Url::parse          scheme, userinfo stripped, host case-folded, path percent-encoded
  → dns::resolve_all    cache, else syscall 572 → up to 4 addresses
  → Fetch               Resolve → Connect → Head → Body, pumped from update() each frame
      ├─ TLS if https   rustls, full verification
      ├─ framing        Content-Length | chunked | to-EOF
      ├─ gzip           Content-Encoding
      └─ charset        header → <meta> → UTF-8
  → htmltext::render    text, links, <base>, title
  → Scrollback          bounded line buffer + cached wrap
```

## Why it stays responsive

`apps/terminal`'s event loop is also its repaint loop. A blocking call inside `on_key` means the
window does not repaint, the cursor does not blink, and **`MSG_WINDOW_CLOSE` is never processed** —
the window looks alive and cannot be closed.

So a page load is a state machine, not a call. `nyx_net::Fetch` is polled once per ~16 ms frame from
`NyxApp::update()`; a short socket timeout is read as "nothing yet, come back next frame" rather
than as failure. Two constants govern it, and **their relationship is the point**:

- `SOCKET_TIMEOUT` (20 ms) — how long one read may block
- `POLL_BUDGET` (120 ms) — how long one poll may spend

`SOCKET_TIMEOUT` **must stay well below** `POLL_BUDGET`. When it was 150 ms — larger than the budget
containing it — the first read that found no data blew the budget and ended the poll, so a transfer
did exactly **one read per frame**: measured at 8 KB/s on a 30 Mbps link, with the network never the
limiting factor. A timeout longer than the budget containing it is a throughput cap wearing a
responsiveness costume.

## Rendering

`libs/htmltext` is a scanner, not a parser — deliberately. It emits structure rather than a flat
wall of text:

```
# Heading                 level is in the marker, so it survives wrapping
  • a list item           nesting indents; <ol> counts instead of bulleting
  > a quotation
  code stays verbatim     inside <pre>, whitespace is content
  cell | cell | cell      table rows keep their column boundaries
  Learn more[1]           the marker goes AFTER the text, so the sentence still reads
```

`<script>` and `<style>` bodies are dropped — they are text nodes, and a naive stripper prints the
whole of jQuery at you. `<base href>` is honoured for link resolution.

Wrapping breaks at spaces with a hanging indent, so a wrapped bullet stays visibly one bullet. A
word longer than the line hard-breaks rather than running off the edge.

## Errors are meant to differ

Phase 9's requirement, and the reason `nyx_net::Error` has the variants it does:

| Condition | What you see |
|---|---|
| Name does not resolve | `cannot resolve example.test` + how to check the link |
| Host will not answer | `142.251.x.x:443 is not answering (…)` |
| Clock wrong | the TLS message **plus** "run `date`, then `time sync`" |
| Untrusted issuer | "does not chain to any trusted root" |
| Wrong hostname | "the certificate is for a different hostname" |
| Redirect loop | `too many redirects` |
| Oversized response | names which limit was hit |
| `mailto:` / `tel:` link | `mailto: links are not pages` |
| HTTP 404/500 | the status, and the body rendered — as a browser should |

"Network error" is not an output this stack produces.

## Limits

| | |
|---|---|
| URL | 8192 bytes |
| Response headers | 64 KB, **128 fields** |
| Body | 8 MB (before *and* after gzip) |
| Redirects | 8 |
| Links per page | 4096 |
| Page into scrollback | 400 000 chars |
| Scrollback | 12 000 lines |
| Whole request | 45 s stepped / 60 s blocking |

## Keys

There are **no modifier chords on this machine** — the kernel builds `pc_keyboard` with
`HandleControl::Ignore` and the key path is a `VecDeque<char>`, so Ctrl and Alt are dropped at the
driver. There is also no scroll wheel. Available: ↑/↓ (command history), PageUp/PageDown/Home/End
(scrollback), Enter, Backspace. Unhandled private-use keys are swallowed rather than typed as boxes.

## Known gaps

- **No connection reuse.** `Connection: close`, so every `open <n>` pays a fresh TCP and TLS
  handshake. Largest remaining performance item.
- **No cookies, no POST, no forms.** Read-only browsing.
- **No JavaScript**, and none planned here.
- **Fonts**: no Indic/CJK glyphs in JetBrains Mono, so those scripts render as boxes. The decoding is
  correct; the face has no coverage.
- **`fetch` blocks.** Use `get`.
