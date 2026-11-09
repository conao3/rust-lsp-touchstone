# lsp-touchstone

A touchstone for the Language Server Protocol: a strict client, a strict server and a checking
proxy for testing LSP implementations against the specification.

- **client** drives any language server from a web UI: send requests, notifications and raw
  frames, open files, and watch every message and finding live.
- **proxy** sits between an editor and a language server and checks every message on the way.
- **server** drives an editor through the server-to-client side of the protocol: progress,
  dynamic registration, configuration, the negotiated position encoding, every completion item
  shape and `workspace/applyEdit`, then checks that the edit the editor applied is what its next
  `didChange` produces.

All three share one set of checks ([docs/rules.md](docs/rules.md)): framing, JSON-RPC, the
lifecycle, params and results against the specification's meta model, capabilities in both
directions, document synchronization, positions the server returns, cancellation and progress.
Where the specification can be read in more than one way, the finding is a warning, not an
error. Types and capability bindings come from the specification's `metaModel.json`
([spec/](spec/)), embedded and interpreted, never written by hand. Position encodings and the
progress state machine are proved correct in Lean through [lean-rs](https://github.com/conao3/lean-rs).

## Usage

### The strict client

```console
$ lsp-touchstone client start-web-ui --port 4317 --record-file session.ndjson
{"Url":"http://127.0.0.1:4317/"}
```

Open the URL, enter the command of a language server (`rust-analyzer`) and press Start. Send
requests, notifications or raw frames from templates or by hand, open files with `didOpen`, and
watch every message in both directions, the server's stderr and the findings on each message as
they happen. Selecting a request highlights its response. Requests from the server
(`workspace/configuration`, `window/workDoneProgress/create` and others) are answered
automatically unless auto-respond is off.

### The checking proxy

Give an editor this command as its language server:

```console
$ lsp-touchstone proxy run-proxy --web-ui-port 4318 --record-file session.ndjson --server-command-args rust-analyzer
```

The proxy starts the real server, forwards every frame in both directions and checks every
message. With `--web-ui-port` it prints the URL of a watch-only web UI to stderr. Frames are
forwarded with their content unchanged and a fresh `Content-Length` header.

### The strict server

```console
$ lsp-touchstone server run-server --web-ui-port 4319 --record-file session.ndjson
```

Use it as the language server of any file type. Each feature is exercised only when the editor
declared the capability for it. Its code actions run commands that send `workspace/applyEdit`.
[docs/editors.md](docs/editors.md) has the configuration for eglot and Neovim and
what each of them did.

### The web UI

The page talks to a GraphQL API at `/api/graphql` (`schema.graphql`); subscriptions use the
`graphql-transport-ws` protocol. The server listens on 127.0.0.1 only and refuses requests whose
`Host` or `Origin` is not its own, so other pages in the browser cannot start processes through
it. `--record-file` appends every message as one JSON line.

### Recorded streams

```console
$ lsp-touchstone framing check-frames --input-file recorded-stream.bin
$ lsp-touchstone --output table framing check-frames --input-file /dev/stdin < recorded-stream.bin
```

`check-frames` reads one direction of a recorded stream and reports each frame's findings.
`--output` is `json` (default), `text` or `table`.

Exit codes: 0 when no error was found, 252 for a usage error, 254 when the input violates the
specification, 255 for any other failure.

## Development

```console
$ nix develop --command make check
$ nix build
$ nix run . -- help
```

`make check-editors` runs eglot and Neovim against the strict server and against rust-analyzer
behind the proxy and prints the findings of each session.

## License

Apache-2.0. `spec/metaModel.json` is from
[microsoft/vscode-languageserver-node](https://github.com/microsoft/vscode-languageserver-node)
under the MIT License.
