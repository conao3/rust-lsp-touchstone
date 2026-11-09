# lsp-touchstone

A touchstone for the Language Server Protocol: a strict client, a strict server and a checking
proxy for testing LSP implementations against the specification.

- **proxy** sits between any client and server and checks every message: framing, JSON-RPC,
  types against the specification's meta model, the lifecycle, capabilities in both directions,
  document synchronization, cancellation and progress.
- **client** drives a server with a script and uses the parts of the protocol that real editors
  rarely exercise (UTF-8 and UTF-32 positions, cancellation, `\r\n` documents).
- **server** drives a client through every server-to-client feature (dynamic registration,
  progress, workspace edits with resource operations, partial results, every completion item
  shape).

Where the specification can be read in more than one way, the finding is a warning, not an
error. Types and rules are generated from the specification's `metaModel.json`
([spec/](spec/)) and never written by hand.

The strict client is driven from a web UI that shows every message live; the checking proxy and
the strict server are not written yet.

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

The page talks to a GraphQL API at `/api/graphql` (`schema.graphql`); subscriptions use the
`graphql-transport-ws` protocol. The server listens on 127.0.0.1 only and refuses requests whose
`Host` or `Origin` is not its own, so other pages in the browser cannot start processes through
it. `--record-file` appends every message as one JSON line.

Checks on the live session: framing, JSON-RPC shape, request and response pairing, and the
lifecycle around `initialize` and `shutdown`.

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

## License

Apache-2.0. `spec/metaModel.json` is from
[microsoft/vscode-languageserver-node](https://github.com/microsoft/vscode-languageserver-node)
under the MIT License.
