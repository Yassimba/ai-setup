#!/usr/bin/env node
import fs from "node:fs";
import { spawn } from "node:child_process";
const file = process.env.FAKE_HERDR_STATE;
const state = (() => { try { return JSON.parse(fs.readFileSync(file, "utf8")); } catch { return { workspace: null, panes: [], agents: [], calls: [] }; } })();
const args = process.argv.slice(2); state.calls.push(args);
const save = () => fs.writeFileSync(file, JSON.stringify(state));
const ok = (result) => { save(); process.stdout.write(JSON.stringify({ result })); };
if (args[0] === "workspace" && args[1] === "list") ok({ workspaces: state.workspace ? [state.workspace] : [] });
else if (args[0] === "workspace" && args[1] === "create") { state.workspace_seq = (state.workspace_seq ?? 0) + 1; state.workspace = { label: args[args.indexOf("--label") + 1], workspace_id: `workspace-${state.workspace_seq}` }; state.panes = [{ pane_id: "seed" }]; ok({ workspace: state.workspace }); }
else if (args[0] === "pane" && args[1] === "list") ok({ panes: state.panes });
else if (args[0] === "agent" && args[1] === "list") ok({ agents: state.agents });
else if (args[0] === "agent" && args[1] === "start") {
 if (!state.workspace || args[args.indexOf("--workspace") + 1] !== state.workspace.workspace_id) { save(); process.stdout.write(JSON.stringify({ error: { message: "workspace missing" } })); process.exitCode = 1; }
 else {
 const separator = args.indexOf("--"), label = args[2], pane_id = `pane-${state.agents.length + 1}`;
 const command = args[separator + 1], commandArgs = args.slice(separator + 2);
 const envelope = commandArgs.at(-1); try { state.envelopeMode = fs.statSync(envelope).mode & 0o777; state.envelope = JSON.parse(fs.readFileSync(envelope, "utf8")); } catch {}
 const child = spawn(command, commandArgs, { cwd: args[args.indexOf("--cwd") + 1], env: process.env, detached: true, stdio: "ignore" }); child.unref();
 state.agents.push({ name: label, pane_id, workspace_id: state.workspace.workspace_id, child_pid: child.pid }); state.panes.push({ pane_id }); save();
 process.stdout.write(JSON.stringify({ result: { agent: { pane_id, tab_id: "tab-1" } } }));
 }
} else if (args[0] === "agent" && args[1] === "get") { const found = state.agents.find((a) => a.name === args[2]); found ? ok({ agent: found }) : (process.stdout.write(JSON.stringify({ error: { message: "missing" } })), process.exitCode = 1); }
else if (args[0] === "agent" && args[1] === "rename") { const found = state.agents.find((a) => a.name === args[2]); if (found) found.name = args[3]; ok({}); }
else if (args[0] === "pane" && args[1] === "close") { const closing = state.agents.find((a) => a.pane_id === args[2]); if (closing?.child_pid) { try { process.kill(-closing.child_pid, "SIGTERM"); } catch {} } state.panes = state.panes.filter((p) => p.pane_id !== args[2]); state.agents = state.agents.filter((a) => a.pane_id !== args[2]); ok({}); }
else if (args[0] === "pane" && args[1] === "move") ok({});
else { save(); process.stdout.write(JSON.stringify({ error: { message: `unsupported ${args.join(" ")}` } })); process.exitCode = 1; }
