// Runs inside the VS Code extension host: starts vscode-languageclient against the command in
// TOUCHSTONE_COMMAND and drives it through the same steps as the other editors.
const vscode = require("vscode");
const { LanguageClient } = require("vscode-languageclient/node");

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

async function tolerate(promise) {
  try {
    return await promise;
  } catch (error) {
    console.log(`request failed: ${error.message}`);
    return undefined;
  }
}

exports.run = async () => {
  const [command, ...args] = JSON.parse(process.env.TOUCHSTONE_COMMAND);
  const file = process.env.TOUCHSTONE_FILE;
  const serverMode = process.env.TOUCHSTONE_MODE === "server";
  const client = new LanguageClient(
    "lsp-touchstone",
    "lsp-touchstone",
    { command, args },
    { documentSelector: [{ scheme: "file", language: "rust" }] },
  );
  await client.start();
  const doc = await vscode.workspace.openTextDocument(file);
  const editor = await vscode.window.showTextDocument(doc);
  await sleep(2000);

  const offset = doc.getText().indexOf("main") + 2;
  const position = doc.positionAt(offset);
  await tolerate(vscode.commands.executeCommand("vscode.executeHoverProvider", doc.uri, position));
  await tolerate(vscode.commands.executeCommand("vscode.executeCompletionItemProvider", doc.uri, position));
  await tolerate(vscode.commands.executeCommand("vscode.executeDefinitionProvider", doc.uri, position));

  await editor.edit((edit) => edit.insert(new vscode.Position(doc.lineCount, 0), "// typed in vscode 😀\n"));
  await sleep(500);
  if (serverMode) {
    const actions = await tolerate(
      vscode.commands.executeCommand("vscode.executeCodeActionProvider", doc.uri, new vscode.Range(0, 0, 0, 0)),
    );
    const action = (actions || []).find((a) => (a.command?.command ?? a.command) === "lsp-touchstone.applyEdit");
    if (action) {
      const cmd = action.command.command ? action.command : action;
      await tolerate(vscode.commands.executeCommand(cmd.command, ...(cmd.arguments || [])));
    }
    await sleep(1500);
  }
  await doc.save();
  await sleep(1000);
  await client.stop();
};
