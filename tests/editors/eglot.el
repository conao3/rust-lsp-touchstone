;;; eglot.el --- Drive eglot against lsp-touchstone in batch  -*- lexical-binding: t; -*-

;; TOUCHSTONE_COMMAND is the language server command as a JSON array, TOUCHSTONE_FILE the file
;; to edit, and TOUCHSTONE_MODE `server' (the strict server) or `proxy' (a real server behind
;; the proxy).

(require 'eglot)

(setq eglot-confirm-server-edits nil
      eglot-sync-connect 10
      eglot-send-changes-idle-time 0.1)

(defun touchstone-wait (seconds)
  (let ((end (+ (float-time) seconds)))
    (while (< (float-time) end)
      (accept-process-output nil 0.05))))

(let* ((command (append (json-parse-string (getenv "TOUCHSTONE_COMMAND") :array-type 'list) nil))
       (file (getenv "TOUCHSTONE_FILE"))
       (server-mode (equal (getenv "TOUCHSTONE_MODE") "server")))
  (setq eglot-server-programs `((prog-mode . ,command)))
  (find-file file)
  (prog-mode)
  (setq-local eglot-workspace-configuration '(:lsp-touchstone (:answer 42)))
  (apply #'eglot (eglot--guess-contact))
  (let ((server (eglot-current-server)))
    (unless server (error "eglot did not connect"))
    (touchstone-wait 2)
    (goto-char (point-min))
    (search-forward "main")
    (backward-char 2)
    ;; A request may fail on the server's side (ContentModified while indexing); that is a
    ;; response, not a failure of the run.
    (ignore-errors (jsonrpc-request server :textDocument/hover (eglot--TextDocumentPositionParams)))
    (ignore-errors (jsonrpc-request server :textDocument/completion (eglot--CompletionParams)))
    (ignore-errors (jsonrpc-request server :textDocument/definition (eglot--TextDocumentPositionParams)))
    (goto-char (point-max))
    (insert "// typed in eglot 😀\n")
    (eglot--signal-textDocument/didChange)
    (touchstone-wait 0.5)
    (when server-mode
      (eglot-execute server `(:command "lsp-touchstone.applyEdit"
                              :arguments [,(plist-get (eglot--TextDocumentIdentifier) :uri)]))
      (touchstone-wait 1)
      (eglot--signal-textDocument/didChange)
      (touchstone-wait 0.5))
    (save-buffer)
    (touchstone-wait 1)
    (eglot-shutdown server)
    (kill-emacs 0)))
