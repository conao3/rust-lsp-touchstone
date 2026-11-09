// Starts a headless VS Code (TOUCHSTONE_VSCODE, run under xvfb-run) with scenario.js as its
// extension tests, in a fresh user data directory next to TOUCHSTONE_FILE.
const path = require("node:path");
const { runTests } = require("@vscode/test-electron");

const project = path.dirname(path.dirname(process.env.TOUCHSTONE_FILE));

runTests({
  vscodeExecutablePath: process.env.TOUCHSTONE_VSCODE,
  extensionDevelopmentPath: __dirname,
  extensionTestsPath: path.join(__dirname, "scenario.js"),
  launchArgs: [
    project,
    "--user-data-dir", path.join(project, ".vscode-user"),
    "--extensions-dir", path.join(project, ".vscode-extensions"),
    "--disable-workspace-trust",
    "--disable-gpu",
  ],
}).catch((error) => {
  console.error(error);
  process.exit(1);
});
