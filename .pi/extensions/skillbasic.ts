import { Type } from "typebox";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import type { Dirent } from "node:fs";
import { readdir, readFile, stat } from "node:fs/promises";
import { basename, dirname, extname, isAbsolute, join, relative, resolve } from "node:path";

/**
 * A small TypeScript Skill BASIC interpreter for pi.
 *
 * This intentionally does not call the Rust interpreter or Ollama. PROMPT
 * arguments are evaluated and returned in the tool result so they enter pi's
 * context directly.
 */

type Value = number | string;
type Vars = Map<string, Value>;

type Statement =
	| { kind: "rem" }
	| { kind: "print"; text: string }
	| { kind: "input"; text: string }
	| { kind: "let"; text: string }
	| { kind: "if"; cond: string; thenText: string }
	| { kind: "goto"; target: number }
	| { kind: "prompt"; expr: string }
	| { kind: "end" }
	| { kind: "stop" };

type ProgramStmt = { line: number; stmt: Statement; source: string };

type ResumeState = {
	pc: number;
	vars: [string, Value][];
	inputPos: number;
	stdout: string;
	prompts: string[];
	instructionCount: number;
};

type RunResult = {
	stdout: string;
	stderr: string;
	prompts: string[];
	vars: Record<string, Value>;
	instructionCount: number;
	stopped: boolean;
	timedOut: boolean;
	needsInput: boolean;
	inputPrompt: string;
	resumeState: ResumeState | null;
};

function splitOutsideQuotes(text: string, separators: Set<string>): string[] {
	const parts: string[] = [];
	let current = "";
	let inString = false;
	for (let i = 0; i < text.length; i++) {
		const ch = text[i];
		if (ch === '"') inString = !inString;
		if (!inString && separators.has(ch)) {
			parts.push(current.trim());
			current = "";
		} else {
			current += ch;
		}
	}
	parts.push(current.trim());
	return parts.filter((p) => p.length > 0);
}

function startsKeyword(text: string, kw: string): boolean {
	return text.toUpperCase() === kw || text.toUpperCase().startsWith(`${kw} `);
}

function parseOneStatement(raw: string): Statement {
	const text = raw.trim();
	const upper = text.toUpperCase();
	if (!text) return { kind: "rem" };
	if (startsKeyword(text, "REM")) return { kind: "rem" };
	if (text === "?" || upper.startsWith("? ")) return { kind: "print", text: text.slice(1).trim() };
	if (startsKeyword(text, "PRINT")) return { kind: "print", text: text.slice(5).trim() };
	if (startsKeyword(text, "INPUT")) return { kind: "input", text: text.slice(5).trim() };
	if (startsKeyword(text, "LET")) return { kind: "let", text: text.slice(3).trim() };
	if (startsKeyword(text, "GOTO")) {
		const n = Number.parseInt(text.slice(4).trim(), 10);
		if (!Number.isFinite(n)) throw new Error(`?SYNTAX ERROR: bad GOTO target: ${raw}`);
		return { kind: "goto", target: n };
	}
	if (startsKeyword(text, "PROMPT")) {
		const expr = text.slice(6).trim();
		if (!expr) throw new Error("?SYNTAX ERROR: PROMPT requires an expression");
		return { kind: "prompt", expr };
	}
	if (startsKeyword(text, "IF")) {
		const rest = text.slice(2).trim();
		const idx = rest.toUpperCase().indexOf(" THEN ");
		if (idx < 0) throw new Error(`?SYNTAX ERROR: IF without THEN: ${raw}`);
		return { kind: "if", cond: rest.slice(0, idx).trim(), thenText: rest.slice(idx + 6).trim() };
	}
	if (upper === "END") return { kind: "end" };
	if (upper === "STOP") return { kind: "stop" };
	if (/^[A-Za-z][A-Za-z0-9]*\$?\s*=/.test(text)) return { kind: "let", text };
	throw new Error(`?SYNTAX ERROR: ${raw}`);
}

function parseProgram(source: string): ProgramStmt[] {
	const out: ProgramStmt[] = [];
	let synthetic = 1_000_000_000;
	for (const rawLine of source.split(/\r?\n/)) {
		const line = rawLine.trim();
		if (!line) continue;
		const m = /^(\d+)\s*(.*)$/.exec(line);
		const lineNo = m ? Number.parseInt(m[1], 10) : synthetic++;
		const body = m ? m[2] : line;
		for (const part of splitOutsideQuotes(body, new Set([":"]))) {
			out.push({ line: lineNo, stmt: parseOneStatement(part), source: part });
		}
	}
	return out.sort((a, b) => a.line - b.line);
}

type Tok = { type: "num" | "str" | "id" | "op" | "eof"; text: string };

function tokenize(expr: string): Tok[] {
	const toks: Tok[] = [];
	let i = 0;
	while (i < expr.length) {
		const ch = expr[i];
		if (/\s/.test(ch)) {
			i++;
			continue;
		}
		if (ch === '"') {
			let j = i + 1;
			let s = "";
			while (j < expr.length && expr[j] !== '"') s += expr[j++];
			if (j >= expr.length) throw new Error("?SYNTAX ERROR: unterminated string");
			toks.push({ type: "str", text: s });
			i = j + 1;
			continue;
		}
		if (/[0-9.]/.test(ch)) {
			let j = i;
			while (j < expr.length && /[0-9.]/.test(expr[j])) j++;
			toks.push({ type: "num", text: expr.slice(i, j) });
			i = j;
			continue;
		}
		if (/[A-Za-z]/.test(ch)) {
			let j = i;
			while (j < expr.length && /[A-Za-z0-9$]/.test(expr[j])) j++;
			toks.push({ type: "id", text: expr.slice(i, j).toUpperCase() });
			i = j;
			continue;
		}
		const two = expr.slice(i, i + 2);
		if (["<=", ">=", "<>"] .includes(two)) {
			toks.push({ type: "op", text: two });
			i += 2;
			continue;
		}
		if ("+-*/^()=<>".includes(ch)) {
			toks.push({ type: "op", text: ch });
			i++;
			continue;
		}
		throw new Error(`?SYNTAX ERROR: unexpected character ${ch}`);
	}
	toks.push({ type: "eof", text: "" });
	return toks;
}

class ExprParser {
	private i = 0;
	constructor(private toks: Tok[], private vars: Vars) {}
	private peek() { return this.toks[this.i]; }
	private take(text?: string) {
		const t = this.peek();
		if (text && t.text !== text) throw new Error(`?SYNTAX ERROR: expected ${text}`);
		this.i++;
		return t;
	}
	parse(): Value {
		const v = this.compare();
		if (this.peek().type !== "eof") throw new Error("?SYNTAX ERROR: trailing expression input");
		return v;
	}
	private compare(): Value {
		let left = this.add();
		while (["=", "<>", "<", ">", "<=", ">="].includes(this.peek().text)) {
			const op = this.take().text;
			const right = this.add();
			const ls = String(left), rs = String(right);
			const ln = Number(left), rn = Number(right);
			const numeric = typeof left === "number" && typeof right === "number";
			let ok = false;
			if (op === "=") ok = numeric ? ln === rn : ls === rs;
			else if (op === "<>") ok = numeric ? ln !== rn : ls !== rs;
			else if (op === "<") ok = numeric ? ln < rn : ls < rs;
			else if (op === ">") ok = numeric ? ln > rn : ls > rs;
			else if (op === "<=") ok = numeric ? ln <= rn : ls <= rs;
			else if (op === ">=") ok = numeric ? ln >= rn : ls >= rs;
			left = ok ? -1 : 0;
		}
		return left;
	}
	private add(): Value {
		let left = this.mul();
		while (["+", "-"].includes(this.peek().text)) {
			const op = this.take().text;
			const right = this.mul();
			if (op === "+") left = typeof left === "string" || typeof right === "string" ? String(left) + String(right) : Number(left) + Number(right);
			else left = Number(left) - Number(right);
		}
		return left;
	}
	private mul(): Value {
		let left = this.unary();
		while (["*", "/"].includes(this.peek().text)) {
			const op = this.take().text;
			const right = this.unary();
			left = op === "*" ? Number(left) * Number(right) : Number(left) / Number(right);
		}
		return left;
	}
	private unary(): Value {
		if (this.peek().text === "-") {
			this.take("-");
			return -Number(this.unary());
		}
		return this.primary();
	}
	private primary(): Value {
		const t = this.peek();
		if (t.type === "num") return Number(this.take().text);
		if (t.type === "str") return this.take().text;
		if (t.type === "id") {
			const id = this.take().text;
			return this.vars.get(id) ?? (id.endsWith("$") ? "" : 0);
		}
		if (t.text === "(") {
			this.take("(");
			const v = this.compare();
			this.take(")");
			return v;
		}
		throw new Error("?SYNTAX ERROR: expected expression");
	}
}

function evalExpr(expr: string, vars: Vars): Value {
	return new ExprParser(tokenize(expr), vars).parse();
}

function truthy(v: Value): boolean {
	return typeof v === "number" ? v !== 0 : v.length > 0;
}

function format(v: Value): string {
	return typeof v === "number" ? String(Number.isInteger(v) ? Math.trunc(v) : v) : v;
}

function assign(text: string, vars: Vars): void {
	const m = /^([A-Za-z][A-Za-z0-9]*\$?)\s*=\s*(.*)$/.exec(text.trim());
	if (!m) throw new Error(`?SYNTAX ERROR: bad assignment: ${text}`);
	const name = m[1].toUpperCase();
	let v = evalExpr(m[2], vars);
	v = name.endsWith("$") ? String(v) : Number(v);
	if (typeof v === "number" && Number.isNaN(v)) throw new Error(`?TYPE MISMATCH: ${name}`);
	vars.set(name, v);
}

function runSkillBasic(source: string, stdin = "", timeoutMs = 120_000, maxInstructions = 100_000, maxPrompts = Number.POSITIVE_INFINITY, resume: ResumeState | null = null): RunResult {
	const program = parseProgram(source);
	const firstIndexByLine = new Map<number, number>();
	program.forEach((s, i) => { if (!firstIndexByLine.has(s.line)) firstIndexByLine.set(s.line, i); });
	const vars: Vars = new Map(resume?.vars ?? []);
	const input = stdin.split(/\r?\n/);
	let inputPos = 0;  // Always start at 0 for fresh stdin on resume
	let stdout = resume?.stdout ?? "";
	let stderr = "";
	const prompts: string[] = resume ? [...resume.prompts] : [];
	let pc = resume?.pc ?? 0;
	let instructionCount = resume?.instructionCount ?? 0;
	let stopped = false;
	let timedOut = false;
	let needsInput = false;
	let inputPrompt = "";
	const deadline = Date.now() + timeoutMs;

	const execInline = (thenText: string): number | null => {
		if (/^\d+$/.test(thenText.trim())) return firstIndexByLine.get(Number(thenText.trim())) ?? null;
		const stmt = parseOneStatement(thenText);
		return execStmt(stmt);
	};

	const execStmt = (stmt: Statement): number | null => {
		switch (stmt.kind) {
			case "rem": return pc + 1;
			case "print": {
				if (!stmt.text) stdout += "\n";
				else stdout += splitOutsideQuotes(stmt.text, new Set([",", ";"])).map((e) => format(evalExpr(e, vars))).join(" ") + "\n";
				return pc + 1;
			}
			case "input": {
				let rest = stmt.text;
				let promptText = "";
				const semicolon = rest.indexOf(";");
				if (rest.trim().startsWith('"') && semicolon >= 0) {
					const promptVal = evalExpr(rest.slice(0, semicolon), vars);
					promptText = String(promptVal);
					rest = rest.slice(semicolon + 1).trim();
				}
				const varNames = rest.split(",").map((s) => s.trim()).filter(Boolean);
				// Check if we have enough input lines for all variables
				const needed = varNames.length;
				const available = input.length - inputPos;
				if (available < needed) {
					// Don't add promptText to stdout when pausing — it will be added on resume
					needsInput = true;
					inputPrompt = promptText;
					stopped = true;
					return null;
				}
				stdout += promptText;
				for (const nameRaw of varNames) {
					const name = nameRaw.toUpperCase();
					const line = input[inputPos++];
					vars.set(name, name.endsWith("$") ? line : Number(line));
				}
				return pc + 1;
			}
			case "let": assign(stmt.text, vars); return pc + 1;
			case "if": return truthy(evalExpr(stmt.cond, vars)) ? execInline(stmt.thenText) : pc + 1;
			case "goto": return firstIndexByLine.get(stmt.target) ?? null;
			case "prompt": {
				const prompt = format(evalExpr(stmt.expr, vars));
				prompts.push(prompt);
				stdout += `[PROMPT] ${prompt}\n`;
				if (prompts.length >= maxPrompts) { stopped = true; return null; }
				return pc + 1;
			}
			case "end": stopped = true; return null;
			case "stop": stopped = true; stdout += `BREAK IN ${program[pc]?.line ?? "?"}\n`; return null;
		}
	};

	try {
		while (pc !== null && pc >= 0 && pc < program.length) {
			if (++instructionCount > maxInstructions) throw new Error(`?EXECUTION ERROR: instruction limit exceeded (${maxInstructions})`);
			if (Date.now() > deadline) { timedOut = true; throw new Error(`?EXECUTION ERROR: timeout after ${timeoutMs}ms`); }
			const next = execStmt(program[pc].stmt);
			if (next === null) break;
			pc = next;
		}
	} catch (e) {
		stderr += e instanceof Error ? e.message : String(e);
		stderr += "\n";
	}

	const resumeState: ResumeState | null = needsInput
		? { pc, vars: Array.from(vars.entries()), inputPos, stdout, prompts, instructionCount }
		: null;
	return { stdout, stderr, prompts, vars: Object.fromEntries(vars), instructionCount, stopped, timedOut, needsInput, inputPrompt, resumeState };
}

function assertInside(root: string, target: string): void {
	const rel = relative(root, target);
	if (rel.startsWith("..") || isAbsolute(rel)) throw new Error(`Refusing to read outside cwd: ${target}`);
}

function isSkillBasicPath(path: string): boolean {
	return [".bas", ".skillbas", ".sbas"].includes(extname(path).toLowerCase());
}

function commandNameFromFile(path: string): string | null {
	const ext = extname(path);
	const name = basename(path, ext);
	return /^[a-z0-9](?:[a-z0-9-]{0,62}[a-z0-9])?$/.test(name) ? name : null;
}

function extractDescription(source: string, fallback: string): string {
	for (const rawLine of source.split(/\r?\n/)) {
		const line = rawLine.trim();
		if (!line) continue;
		const body = line.replace(/^\d+\s*/, "").trim();
		if (!startsKeyword(body, "REM")) continue;
		const description = body.slice(3).trim();
		if (description) return description;
	}
	return fallback;
}

async function discoverSkillbasFiles(cwd: string): Promise<Array<{ name: string; path: string; description: string }>> {
	const dir = resolve(cwd, ".pi", "skillbas");
	let entries: Dirent[];
	try {
		entries = await readdir(dir, { withFileTypes: true });
	} catch {
		return [];
	}

	const byName = new Map<string, { name: string; path: string; description: string; preferred: boolean }>();
	for (const entry of entries) {
		if (!entry.isFile()) continue;
		const full = join(dir, entry.name);
		const extension = extname(entry.name).toLowerCase();
		if (extension !== ".skillbas" && extension !== ".sbas") continue;
		const name = commandNameFromFile(entry.name);
		if (!name) {
			console.warn(`Skipping Skill BASIC command with invalid name: ${entry.name}`);
			continue;
		}
		const source = await readFile(full, "utf8");
		const item = {
			name,
			path: full,
			description: extractDescription(source, `Run ${entry.name} as a Skill BASIC prompt skill`),
			preferred: extension === ".sbas",
		};
		const existing = byName.get(name);
		if (!existing || (!existing.preferred && item.preferred)) byName.set(name, item);
	}

	return Array.from(byName.values()).sort((a, b) => a.name.localeCompare(b.name));
}

const skillbasTool = {
	name: "skillbas",
	label: "Skill BASIC TS",
	description: "Interpret Skill BASIC code directly in TypeScript without calling the Rust interpreter. PROMPT arguments are returned in context instead of sent to Ollama.",
	promptSnippet: "Interpret Skill BASIC .bas code directly and expose PROMPT text in context",
	promptGuidelines: [
		"Use skillbas to run small Skill BASIC snippets or .bas files when the user wants PROMPT arguments in pi context without calling the Rust interpreter.",
		"For Skill BASIC INPUT statements, pass newline-separated input with skillbas.stdin.",
		"When the result says 'NEEDS INPUT', ask the user for input and call skillbas again with the same code/path, the resumeState from the result, and the new stdin. Repeat until the program finishes.",
	],
	parameters: Type.Object({
		code: Type.Optional(Type.String({ description: "Inline Skill BASIC source code" })),
		path: Type.Optional(Type.String({ description: "Path to a .bas file, relative to the current project" })),
		stdin: Type.Optional(Type.String({ description: "Optional newline-separated stdin for INPUT" })),
		resumeState: Type.Optional(Type.Any({ description: "Resume state from a previous run that returned needsInput. Pass this along with new stdin to continue an interactive program." })),
		timeoutMs: Type.Optional(Type.Number({ description: "Execution timeout in milliseconds; default 120000" })),
		maxInstructions: Type.Optional(Type.Number({ description: "Maximum executed statements; default 100000" })),
	}),
	async execute(_toolCallId, params, _signal, onUpdate, ctx) {
		if (!params.code && !params.path) throw new Error("Provide either code or path");
		if (params.code && params.path) throw new Error("Provide code or path, not both");

		let source = params.code ?? "";
		let label = "inline code";
		if (params.path) {
			const full = isAbsolute(params.path) ? resolve(params.path) : resolve(ctx.cwd, params.path);
			assertInside(ctx.cwd, full);
			if (!isSkillBasicPath(full)) throw new Error(`Expected a .bas, .sbas, or .skillbas file: ${params.path}`);
			const info = await stat(full);
			if (!info.isFile()) throw new Error(`Not a file: ${params.path}`);
			source = await readFile(full, "utf8");
			label = relative(ctx.cwd, full);
		}

		onUpdate?.({ content: [{ type: "text", text: `Interpreting Skill BASIC ${label}...` }] });
		const result = runSkillBasic(
			source,
			params.stdin ?? "",
			Math.max(1000, Math.min(params.timeoutMs ?? 120_000, 600_000)),
			Math.max(1, Math.min(params.maxInstructions ?? 100_000, 1_000_000)),
			Number.POSITIVE_INFINITY,
			params.resumeState ?? null,
		);

		if (result.needsInput && result.resumeState) {
			const resumeJson = JSON.stringify(result.resumeState);
			const text = [
				`Skill BASIC TS run: ${label}`,
				`instructions: ${result.instructionCount}`,
				`⚠️ NEEDS INPUT — program paused at INPUT statement`,
				result.inputPrompt ? `input prompt: ${result.inputPrompt}` : "input prompt: (none)",
				result.stdout ? `stdout so far:\n${result.stdout}` : "stdout: <empty>",
				`\nTo continue: call skillbas again with the same ${params.code ? "code" : "path"}, pass the resumeState JSON below as the resumeState parameter, and provide the user's input as stdin.`,
				`resumeState: ${resumeJson}`,
			].join("\n");
			return { content: [{ type: "text", text }], details: result };
		}

		const text = [
			`Skill BASIC TS run: ${label}`,
			`instructions: ${result.instructionCount}`,
			result.prompts.length ? `prompts:\n${result.prompts.map((p, i) => `${i + 1}. ${p}`).join("\n")}` : "prompts: <none>",
			result.stdout ? `stdout:\n${result.stdout}` : "stdout: <empty>",
			result.stderr ? `stderr:\n${result.stderr}` : "stderr: <empty>",
		].join("\n");

		return { content: [{ type: "text", text }], details: result };
	},
};

export default async function skillBasicTsExtension(pi: ExtensionAPI) {
	pi.registerTool(skillbasTool);

	for (const skill of await discoverSkillbasFiles(process.cwd())) {
		pi.registerCommand(`skillbas:${skill.name}`, {
			description: skill.description,
			handler: async (args, ctx) => {
				const source = await readFile(skill.path, "utf8");
				const result = runSkillBasic(source, args ? `${args}\n` : "", 30_000, 10_000, 1);
				if (result.stderr) {
					ctx.ui.notify(result.stderr.trim(), "error");
					return;
				}

				const promptText = result.prompts.join("\n\n").trim();
				if (!promptText) {
					ctx.ui.notify(`No PROMPT output produced by /skillbas:${skill.name}`, "warning");
					return;
				}

				const location = relative(ctx.cwd, skill.path);
				const skillBlock = [
					`<skill name="skillbas:${skill.name}" location="${location}">`,
					`References are relative to ${relative(ctx.cwd, dirname(skill.path)) || "."}.`,
					"",
					promptText,
					"</skill>",
				].join("\n");

				if (ctx.isIdle()) pi.sendUserMessage(skillBlock);
				else pi.sendUserMessage(skillBlock, { deliverAs: "followUp" });
			},
		});
	}

	// Make the tool active after reload/startup even if the current session's
	// active-tool list was created before this extension existed.
	pi.on("session_start", async () => {
		const active = new Set(pi.getActiveTools());
		active.add("skillbas");
		pi.setActiveTools(Array.from(active));
	});
}
