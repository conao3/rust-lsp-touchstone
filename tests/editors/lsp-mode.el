;;; lsp-mode.el --- Drive lsp-mode against lsp-touchstone in batch  -*- lexical-binding: t; -*-

;; TOUCHSTONE_COMMAND is the language server command as a JSON array, TOUCHSTONE_FILE the file
;; to edit, and TOUCHSTONE_MODE `server' (the strict server) or `proxy' (a real server behind
;; the proxy).

;; Load the autoloads as package.el does for an installed lsp-mode.
(load "lsp-mode-autoloads" nil t)
(require 'lsp-mode)

(defun touchstone-wait (seconds)
  (let ((end (+ (float-time) seconds)))
    (while (< (float-time) end)
      (accept-process-output nil 0.05))))

(defun touchstone-wait-until (seconds predicate)
  (let ((end (+ (float-time) seconds)))
    (while (and (< (float-time) end) (not (funcall predicate)))
      (accept-process-output nil 0.05))
    (funcall predicate)))

(let* ((command (append (json-parse-string (getenv "TOUCHSTONE_COMMAND") :array-type 'list) nil))
       (file (getenv "TOUCHSTONE_FILE"))
       (server-mode (equal (getenv "TOUCHSTONE_MODE") "server")))
  (setq lsp-session-file (expand-file-name "lsp-session" (file-name-directory file))
        lsp-auto-guess-root t
        lsp-guess-root-without-session t
        lsp-keep-workspace-alive nil
        lsp-restart 'ignore
        lsp-enable-file-watchers t
        lsp-file-watch-threshold nil
        lsp-warn-no-matched-clients nil
        lsp-enable-suggest-server-download nil)
  (add-to-list 'lsp-language-id-configuration '(prog-mode . "rust"))
  (lsp-register-client
   (make-lsp-client :new-connection (lsp-stdio-connection command)
                    :major-modes '(prog-mode)
                    :priority 100
                    :server-id 'lsp-touchstone))
  (find-file file)
  (prog-mode)
  (lsp)
  (unless (touchstone-wait-until
           20 (lambda ()
                (let ((workspace (car (lsp-workspaces))))
                  (and workspace (eq (lsp--workspace-status workspace) 'initialized)))))
    (error "lsp-mode did not initialize"))
  (touchstone-wait 2)
  (goto-char (point-min))
  (search-forward "main")
  (backward-char 2)
  ;; A request may fail on the server's side (ContentModified while indexing); that is a
  ;; response, not a failure of the run.
  (dolist (method '("textDocument/hover" "textDocument/completion" "textDocument/definition"))
    (ignore-errors (lsp-request method (lsp--text-document-position-params))))
  (goto-char (point-max))
  (insert "// typed in lsp-mode 😀\n")
  (touchstone-wait 0.5)
  (when server-mode
    (ignore-errors
      (lsp-request "workspace/executeCommand"
                   (list :command "lsp-touchstone.applyEdit"
                         :arguments (vector (lsp--buffer-uri)))))
    (touchstone-wait 1))
  (save-buffer)
  (touchstone-wait 1)
  (let ((workspace (car (lsp-workspaces))))
    (lsp-workspace-shutdown workspace)
    (touchstone-wait-until 10 (lambda () (not (process-live-p (lsp--workspace-proc workspace))))))
  (kill-emacs 0))
