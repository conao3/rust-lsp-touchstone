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

The current release checks the base protocol framing only.

## Usage

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
