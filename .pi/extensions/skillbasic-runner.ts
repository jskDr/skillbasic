import { Type } from "@earendil-works/pi-ai";
import { defineTool, type ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { spawn } from "node:child_process";
import { access, readFile, stat } from "node:fs/promises";
import { constants } from "node:fs";
import { dirname, extname, isAbsolute, relative, resolve } from "node:path";

async function findSkillBasicRoot(start: string): Promise<string> {
	let dir = resolve(start);
	while (true) {
		const cargoToml = resolve(dir, "Cargo.toml");
		try {
			await access(cargoToml, constants.R_OK);
			const text = await readFile(cargoToml, "utf8");
			if (/^name\s*=\s*["']skillbasic["']/m.test(text)) return dir;
		} catch {
			// Keep walking upward.
		}

		const parent = dirname(dir);
		if (parent === dir) break;
		dir = parent;
	}

	throw new Error("Could not find the skillbasic Cargo project root from the current directory");
}

function assertInside(root: string, target: string): void {
	const rel = relative(root, target);
	if (rel.startsWith("..") || isAbsolute(rel)) {
		throw new Error(`Refusing to run a file outside the skillbasic project: ${target}`);
	}
}

function runProcess(
	command: string,
	args: string[],
	options: { cwd: string; stdin?: string; timeoutMs: number; signal?: AbortSignal; env?: NodeJS.ProcessEnv },
): Promise<{ code: number | null; signal: NodeJS.Signals | null; stdout: string; stderr: string; timedOut: boolean }> {
	return new Promise((resolvePromise, reject) => {
		const child = spawn(command, args, {
			cwd: options.cwd,
			env: { ...process.env, ...options.env },
			stdio: ["pipe", "pipe", "pipe"],
		});

		let stdout = "";
		let stderr = "";
		let settled = false;
		let timedOut = false;

		const finish = (result: { code: number | null; signal: NodeJS.Signals | null }) => {
			if (settled) return;
			settled = true;
			clearTimeout(timer);
			options.signal?.removeEventListener("abort", abort);
			resolvePromise({ ...result, stdout, stderr, timedOut });
		};

		const abort = () => {
			if (settled) return;
			child.kill("SIGTERM");
			setTimeout(() => {
				if (!settled) child.kill("SIGKILL");
			}, 1000).unref();
		};

		const timer = setTimeout(() => {
			timedOut = true;
			abort();
		}, options.timeoutMs);

		options.signal?.addEventListener("abort", abort);

		child.stdout.on("data", (chunk) => {
			stdout += chunk.toString();
		});
		child.stderr.on("data", (chunk) => {
			stderr += chunk.toString();
		});
		child.on("error", reject);
		child.on("close", (code, signal) => finish({ code, signal }));

		child.stdin.end(options.stdin ?? "");
	});
}

const runSkillBasicTool = defineTool({
	name: "run_skillbasic",
	label: "Run Skill BASIC",
	description: "Run a Skill BASIC .bas program in this project and return stdout/stderr.",
	promptSnippet: "Run Skill BASIC .bas programs with the project interpreter",
	promptGuidelines: [
		"Use run_skillbasic when the user asks to execute a Skill BASIC .bas program.",
		"For Skill BASIC programs that use INPUT, pass newline-separated input with the stdin parameter.",
	],
	parameters: Type.Object({
		path: Type.String({ description: "Path to a .bas file, relative to the project root or current directory" }),
		stdin: Type.Optional(Type.String({ description: "Optional stdin to feed to the program" })),
		tracePrompts: Type.Optional(
			Type.Boolean({ description: "Include evaluated PROMPT arguments in stderr/tool result; default true" }),
		),
		timeoutMs: Type.Optional(Type.Number({ description: "Execution timeout in milliseconds; default 120000" })),
	}),
	async execute(_toolCallId, params, signal, onUpdate, ctx) {
		const root = await findSkillBasicRoot(ctx.cwd);
		const programPath = isAbsolute(params.path) ? resolve(params.path) : resolve(ctx.cwd, params.path);
		assertInside(root, programPath);

		if (extname(programPath).toLowerCase() !== ".bas") {
			throw new Error(`Expected a .bas file, got: ${params.path}`);
		}

		const info = await stat(programPath);
		if (!info.isFile()) throw new Error(`Not a file: ${params.path}`);

		const timeoutMs = Math.max(1000, Math.min(params.timeoutMs ?? 120_000, 600_000));
		onUpdate?.({ content: [{ type: "text", text: `Running ${relative(root, programPath)}...` }] });

		const started = Date.now();
		const result = await runProcess("cargo", ["run", "--quiet", "--", programPath], {
			cwd: root,
			stdin: params.stdin,
			timeoutMs,
			signal,
			env: params.tracePrompts === false ? {} : { SKILLBASIC_TRACE_PROMPTS: "1" },
		});
		const durationMs = Date.now() - started;

		const status = result.timedOut
			? `timed out after ${timeoutMs}ms`
			: result.code === 0
				? "succeeded"
				: `exited with code ${result.code ?? `signal ${result.signal}`}`;

		const text = [
			`Skill BASIC run ${status} (${durationMs}ms): ${relative(root, programPath)}`,
			result.stdout ? `\nstdout:\n${result.stdout}` : "\nstdout: <empty>",
			result.stderr ? `\nstderr:\n${result.stderr}` : "\nstderr: <empty>",
		].join("\n");

		return {
			content: [{ type: "text", text }],
			details: {
				path: relative(root, programPath),
				code: result.code,
				signal: result.signal,
				timedOut: result.timedOut,
				durationMs,
				stdout: result.stdout,
				stderr: result.stderr,
			},
		};
	},
});

export default function skillBasicRunnerExtension(pi: ExtensionAPI) {
	pi.registerTool(runSkillBasicTool);

	pi.registerCommand("runbas", {
		description: "Run a Skill BASIC .bas file with cargo run -- <file>",
		handler: async (args, ctx) => {
			const path = args.trim();
			if (!path) {
				ctx.ui.notify("Usage: /runbas samples/hello.bas", "warning");
				return;
			}
			const root = await findSkillBasicRoot(ctx.cwd);
			const programPath = isAbsolute(path) ? resolve(path) : resolve(ctx.cwd, path);
			assertInside(root, programPath);
			const result = await runProcess("cargo", ["run", "--quiet", "--", programPath], {
				cwd: root,
				timeoutMs: 120_000,
				env: { SKILLBASIC_TRACE_PROMPTS: "1" },
			});
			ctx.ui.notify(
				`runbas ${result.code === 0 ? "succeeded" : "failed"}\n${result.stdout}${result.stderr ? `\nstderr:\n${result.stderr}` : ""}`,
				result.code === 0 ? "info" : "error",
			);
		},
	});
}
