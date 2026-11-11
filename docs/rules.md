# Rules

Every finding lsp-touchstone reports, by area. An **error** is a behavior the specification
forbids. A **warning** is a behavior the specification allows or leaves open but that
implementations read differently; its row says which reading lsp-touchstone takes.

The specification is LSP 3.18 (`spec/metaModel.json`) and JSON-RPC 2.0. A test fails when a code
used in `src/` is missing here or a code here is no longer used.

## Framing

The base protocol: a header part of `Name: value\r\n` fields ended by `\r\n`, then the content.

| Code | Severity | Rule |
|---|---|---|
| `framing/missing-content-length` | error | Every header part has `Content-Length`. The stream cannot be read further. |
| `framing/content-length-unreadable` | error | `Content-Length` is a length. The stream cannot be read further. |
| `framing/content-length-not-a-number` | error | `Content-Length` is a decimal number without a sign. |
| `framing/duplicate-content-length` | error | `Content-Length` appears once. |
| `framing/duplicate-content-type` | error | `Content-Type` appears at most once. |
| `framing/header-line-terminator` | error | Header lines end with `\r\n`. |
| `framing/header-field-separator` | error | Name and value are separated by `: `. |
| `framing/header-not-ascii` | error | The header part is ASCII. |
| `framing/unknown-header-field` | warning | The specification defines `Content-Length` and `Content-Type` only. |
| `framing/content-type-mime` | warning | The type is `application/vscode-jsonrpc`. |
| `framing/charset-utf8-alias` | warning | The charset is `utf-8`; `utf8` is accepted for backwards compatibility. |
| `framing/charset-unsupported` | error | Only `utf-8` is supported. |
| `framing/content-not-utf8` | error | The content is UTF-8. |
| `framing/content-not-json` | error | The content is exactly one JSON value of `Content-Length` bytes (a byte count, not a character count). |
| `framing/unexpected-eof-in-header` | error | The stream does not end inside a header part. |
| `framing/unexpected-eof-in-body` | error | The stream does not end inside the content. |

## JSON-RPC

| Code | Severity | Rule |
|---|---|---|
| `jsonrpc/not-json` | error | The content parses as JSON. |
| `jsonrpc/not-an-object` | error | A message is an object; LSP uses no batches. |
| `jsonrpc/version` | error | `"jsonrpc": "2.0"`. |
| `jsonrpc/method-not-a-string` | error | `method` is a string. |
| `jsonrpc/params-structure` | error | `params` is an object or an array when present (never `null`). |
| `jsonrpc/request-id-type` | error | A request id is an integer or a string. |
| `jsonrpc/response-id-type` | error | A response id is an integer, a string or `null`. |
| `jsonrpc/result-and-error` | error | A response has `result` or `error`, not both. |
| `jsonrpc/response-without-result` | error | A response has `result` or `error`; success without a value is `"result": null`. |
| `jsonrpc/error-not-an-object` | error | `error` is an object. |
| `jsonrpc/error-code` | error | `error.code` is an integer. |
| `jsonrpc/error-message` | error | `error.message` is a string. |
| `jsonrpc/unknown-shape` | error | A message is a request, a notification or a response. |
| `jsonrpc/duplicate-request-id` | error | A side does not reuse the id of its own request that is still waiting. |
| `jsonrpc/response-to-unknown-request` | error | A response answers a waiting request of the other side. |
| `jsonrpc/request-never-answered` | error | Every request gets a response before the session ends, also when the handler fails or the request was cancelled. |

## Lifecycle

| Code | Severity | Rule |
|---|---|---|
| `lifecycle/client-message-before-initialize-result` | error | Until the server answers `initialize`, the client sends no other request or notification (except `exit`); responses to the server's requests are allowed. |
| `lifecycle/server-message-before-initialize-result` | error | Until it answers `initialize`, the server sends only `window/showMessage`, `window/logMessage`, `telemetry/event`, `window/showMessageRequest` and `$/progress`. |
| `lifecycle/client-message-after-shutdown` | error | After `shutdown`, the client sends no request or notification but `exit`; responses to the server's requests are allowed. |
| `lifecycle/shutdown-not-sent` | warning | A client that is done with the server sends `shutdown` before `exit`; a session that ends without it left the server to be ended another way. |
| `lifecycle/exit-not-sent` | warning | After `shutdown` the client sends `exit`; a client that kills the server instead skips the server's own cleanup. |

## Types

Params and results are validated against the meta model: structures with their `extends` and
`mixins`, literals, unions, enumerations, type aliases, arrays, maps and tuples.

| Code | Severity | Rule |
|---|---|---|
| `types/unknown-method` | warning | The method is defined by the specification. Methods starting with `$/` may be ignored and are not reported. |
| `types/wrong-direction` | error | The message goes in the direction the specification gives it. |
| `types/wrong-message-kind` | error | A request method is sent as a request and a notification method as a notification. |
| `types/missing-params` | error | A method with params receives them. |
| `types/unexpected-params` | warning | A method without params receives none (`exit` with `{}` is common). |
| `types/missing-property` | error | Required properties are present. |
| `types/null-for-optional-property` | error | An optional property is omitted, not `null`, unless its type includes `null`. |
| `types/unknown-property` | warning | Properties are those of the type; extensions (`$streamingDiagnostics`, `experimental` contents) are reported as warnings. |
| `types/wrong-type` | error | A value has its base type: `integer` in [-2^31, 2^31-1], `uinteger` in [0, 2^31-1], strings, booleans, `null`, arrays, objects. |
| `types/wrong-literal` | error | A string literal type has exactly its value. |
| `types/unknown-enum-value` | error | An enumeration value is one of its values unless the enumeration supports custom values. |
| `types/no-alternative-matches` | error | A union value matches one of its alternatives. |
| `types/unknown-type` | error | The meta model defines every referenced type (an error of lsp-touchstone itself). |

## Capabilities

| Code | Severity | Rule |
|---|---|---|
| `capability/server-did-not-declare` | error for requests, warning for notifications | The client sends a method only when the server declared its `serverCapability` or registered it dynamically. A `textDocumentSync` number declares open, close and change. |
| `capability/client-did-not-declare` | error for requests, warning for notifications | The server sends a method only when the client declared its `clientCapability`. Notifications such as `window/showMessage` are commonly sent regardless, hence the warning. |
| `capability/dynamic-registration-not-supported` | error | The server registers a method dynamically only when the client declared `dynamicRegistration` for it. |
| `capability/position-encoding-not-offered` | error | The server chooses a position encoding the client offered, or `utf-16`. |
| `capability/unknown-position-encoding` | warning | lsp-touchstone knows `utf-8`, `utf-16` and `utf-32`; positions in other encodings are read as `utf-16`. |

## Document synchronization

lsp-touchstone keeps the text of every open document as the server should see it.

| Code | Severity | Rule |
|---|---|---|
| `sync/already-open` | error | A document is opened once until it is closed. |
| `sync/not-open` | error | A document is changed, saved and closed only while open. |
| `sync/version-not-increasing` | error | The version increases with every change. |
| `sync/incremental-not-negotiated` | error | Changes with a range are sent only when the server asked for incremental sync. |
| `sync/position-splits-character` | error | A change position is on a character boundary in the negotiated encoding. |
| `sync/line-out-of-range` | error | A change position is on an existing line. |
| `sync/character-past-line-end` | warning | A character past the end of its line is clamped to the line length, as the specification says. |
| `sync/range-reversed` | error | A change range starts before it ends. |
| `sync/range-length-mismatch` | warning | The deprecated `rangeLength` matches the range. |
| `sync/save-text-mismatch` | error | The text of `didSave` equals the text the changes produce. |
| `sync/diagnostics-for-future-version` | error | Diagnostics are not for a version the client has not sent. |
| `sync/diagnostics-for-old-version` | warning | Diagnostics for an older version are stale; they are allowed while changes are in flight. |

## Positions from the server

Every range in diagnostics, in `workspace/applyEdit` and in results of requests on a document is
checked against the open document it refers to, when the document has not changed since the
request.

| Code | Severity | Rule |
|---|---|---|
| `positions/splits-character` | error | A position is on a character boundary in the negotiated encoding. |
| `positions/line-out-of-range` | error | A position is on an existing line. |
| `positions/character-past-line-end` | warning | A character past the end of its line is clamped to the line length. |
| `positions/range-reversed` | error | A range starts before it ends. |

## Cancellation

| Code | Severity | Rule |
|---|---|---|
| `cancel/unknown-request` | warning | `$/cancelRequest` names a waiting request of the sender; the response may have crossed it. |
| `cancel/cancelled-without-request` | warning | `RequestCancelled` answers a request that was cancelled. |

## Progress

The token states follow `src/progress.rs`, proved in Lean: `begin`, any number of `report`s, then
`end`, and nothing after `end`.

| Code | Severity | Rule |
|---|---|---|
| `progress/unknown-token` | error | Progress uses a token created by `window/workDoneProgress/create` or given in a request. A token whose creation failed must not be used. |
| `progress/token-already-exists` | error | A created token is not created again while in use. |
| `progress/before-create-response` | warning | Progress waits for the client to answer `window/workDoneProgress/create`; if the answer is an error, progress already sent broke the rule. |
| `progress/unknown-kind` | error | The value's kind is `begin`, `report` or `end`. |
| `progress/out-of-order` | error | A token goes through `begin`, `report`*, `end` once. |

## Client behavior seen by the strict server

| Code | Severity | Rule |
|---|---|---|
| `client/apply-edit-not-reflected` | error | After the client answers `workspace/applyEdit` with `applied: true`, its next `didChange` produces the edited text. |
| `client/apply-edit-refused` | warning | The client did not apply an edit. |
| `client/configuration-length-mismatch` | error | `workspace/configuration` returns one result per item. |
