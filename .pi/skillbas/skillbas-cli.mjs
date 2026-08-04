#!/usr/bin/env node
/**
 * Standalone Skill BASIC interpreter CLI with interactive INPUT support.
 *
 * Usage:
 *   node skillbas-cli.mjs <file.sbas> [--stdin TEXT] [--resume JSON] [--max-instructions N] [--timeout MS]
 *
 * When the program pauses at an INPUT statement (stdin exhausted), the output
 * includes "---NEEDS INPUT---" and a resumeState JSON string. Pass that JSON
 * back via --resume along with new --stdin to continue.
 */

import { readFile } from "node:fs/promises";
import { resolve } from "node:path";

// ── Parser ──

function splitOutsideQuotes(text, separators) {
	const parts = [];
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

function startsKeyword(text, kw) {
	return text.toUpperCase() === kw || text.toUpperCase().startsWith(kw + " ");
}

function parseOneStatement(raw) {
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
		if (!Number.isFinite(n)) throw new Error("?SYNTAX ERROR: bad GOTO target: " + raw);
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
		if (idx < 0) throw new Error("?SYNTAX ERROR: IF without THEN: " + raw);
		return { kind: "if", cond: rest.slice(0, idx).trim(), thenText: rest.slice(idx + 6).trim() };
	}
	if (upper === "END") return { kind: "end" };
	if (upper === "STOP") return { kind: "stop" };
	if (/^[A-Za-z][A-Za-z0-9]*\$?\s*=/.test(text)) return { kind: "let", text };
	throw new Error("?SYNTAX ERROR: " + raw);
}

function parseProgram(source) {
	const out = [];
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

// ── Expression evaluator ──

function tokenize(expr) {
	const toks = [];
	let i = 0;
	while (i < expr.length) {
		const ch = expr[i];
		if (/\s/.test(ch)) { i++; continue; }
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
		if (["<=", ">=", "<>"].includes(two)) { toks.push({ type: "op", text: two }); i += 2; continue; }
		if ("+-*/^()=<>".includes(ch)) { toks.push({ type: "op", text: ch }); i++; continue; }
		throw new Error("?SYNTAX ERROR: unexpected character " + ch);
	}
	toks.push({ type: "eof", text: "" });
	return toks;
}

class ExprParser {
	constructor(toks, vars) {
		this.toks = toks;
		this.vars = vars;
		this.i = 0;
	}
	peek() { return this.toks[this.i]; }
	take(text) {
		const t = this.peek();
		if (text && t.text !== text) throw new Error("?SYNTAX ERROR: expected " + text);
		this.i++;
		return t;
	}
	parse() {
		const v = this.compare();
		if (this.peek().type !== "eof") throw new Error("?SYNTAX ERROR: trailing expression input");
		return v;
	}
	compare() {
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
	add() {
		let left = this.mul();
		while (["+", "-"].includes(this.peek().text)) {
			const op = this.take().text;
			const right = this.mul();
			if (op === "+") left = typeof left === "string" || typeof right === "string" ? String(left) + String(right) : Number(left) + Number(right);
			else left = Number(left) - Number(right);
		}
		return left;
	}
	mul() {
		let left = this.unary();
		while (["*", "/"].includes(this.peek().text)) {
			const op = this.take().text;
			const right = this.unary();
			left = op === "*" ? Number(left) * Number(right) : Number(left) / Number(right);
		}
		return left;
	}
	unary() {
		if (this.peek().text === "-") { this.take("-"); return -Number(this.unary()); }
		return this.primary();
	}
	primary() {
		const t = this.peek();
		if (t.type === "num") return Number(this.take().text);
		if (t.type === "str") return this.take().text;
		if (t.type === "id") {
			const id = this.take().text;
			return this.vars.get(id) ?? (id.endsWith("$") ? "" : 0);
		}
		if (t.text === "(") { this.take("("); const v = this.compare(); this.take(")"); return v; }
		throw new Error("?SYNTAX ERROR: expected expression");
	}
}

function evalExpr(expr, vars) {
	return new ExprParser(tokenize(expr), vars).parse();
}

function truthy(v) {
	return typeof v === "number" ? v !== 0 : v.length > 0;
}

function formatVal(v) {
	return typeof v === "number" ? String(Number.isInteger(v) ? Math.trunc(v) : v) : v;
}

function assign(text, vars) {
	const m = /^([A-Za-z][A-Za-z0-9]*\$?)\s*=\s*(.*)$/.exec(text.trim());
	if (!m) throw new Error("?SYNTAX ERROR: bad assignment: " + text);
	const name = m[1].toUpperCase();
	let v = evalExpr(m[2], vars);
	v = name.endsWith("$") ? String(v) : Number(v);
	if (typeof v === "number" && Number.isNaN(v)) throw new Error("?TYPE MISMATCH: " + name);
	vars.set(name, v);
}

// ── Interpreter ──

function runSkillBasic(source, stdin, timeoutMs, maxInstructions, resume) {
	const program = parseProgram(source);
	const firstIndexByLine = new Map();
	program.forEach((s, i) => { if (!firstIndexByLine.has(s.line)) firstIndexByLine.set(s.line, i); });
	const vars = new Map(resume?.vars ?? []);
	const input = stdin.split(/\r?\n/);
	let inputPos = resume ? 0 : 0;  // Always start at 0 for fresh/new stdin on resume
	let stdout = resume?.stdout ?? "";
	let stderr = "";
	const prompts = resume ? [...resume.prompts] : [];
	let pc = resume?.pc ?? 0;
	let instructionCount = resume?.instructionCount ?? 0;
	let stopped = false;
	let timedOut = false;
	let needsInput = false;
	let inputPrompt = "";
	const deadline = Date.now() + timeoutMs;

	const execInline = (thenText) => {
		if (/^\d+$/.test(thenText.trim())) return firstIndexByLine.get(Number(thenText.trim())) ?? null;
		const stmt = parseOneStatement(thenText);
		return execStmt(stmt);
	};

	const execStmt = (stmt) => {
		switch (stmt.kind) {
			case "rem": return pc + 1;
			case "print": {
				if (!stmt.text) stdout += "\n";
				else stdout += splitOutsideQuotes(stmt.text, new Set([",", ";"])).map((e) => formatVal(evalExpr(e, vars))).join(" ") + "\n";
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
				const prompt = formatVal(evalExpr(stmt.expr, vars));
				prompts.push(prompt);
				stdout += "[PROMPT] " + prompt + "\n";
				return pc + 1;
			}
			case "end": stopped = true; return null;
			case "stop": stopped = true; stdout += "BREAK IN " + (program[pc]?.line ?? "?") + "\n"; return null;
		}
	};

	try {
		while (pc !== null && pc >= 0 && pc < program.length) {
			if (++instructionCount > maxInstructions) throw new Error("?EXECUTION ERROR: instruction limit exceeded (" + maxInstructions + ")");
			if (Date.now() > deadline) { timedOut = true; throw new Error("?EXECUTION ERROR: timeout after " + timeoutMs + "ms"); }
			const next = execStmt(program[pc].stmt);
			if (next === null) break;
			pc = next;
		}
	} catch (e) {
		stderr += (e instanceof Error ? e.message : String(e)) + "\n";
	}

	const resumeState = needsInput
		? { pc, vars: Array.from(vars.entries()), inputPos, stdout, prompts, instructionCount }
		: null;
	return { stdout, stderr, prompts, vars: Object.fromEntries(vars), instructionCount, stopped, timedOut, needsInput, inputPrompt, resumeState };
}

// ── CLI ──

function parseArgs(argv) {
	const args = argv.slice(2);
	let path = "";
	let stdin = "";
	let resume = null;
	let maxInstructions = 100_000;
	let timeoutMs = 120_000;

	for (let i = 0; i < args.length; i++) {
		if (args[i] === "--stdin" || args[i] === "-s") { stdin = args[++i] ?? ""; continue; }
		if (args[i] === "--resume" || args[i] === "-r") { resume = args[++i] ?? null; continue; }
		if (args[i] === "--max-instructions" || args[i] === "-m") { maxInstructions = Number(args[++i]) || 100_000; continue; }
		if (args[i] === "--timeout" || args[i] === "-t") { timeoutMs = Number(args[++i]) || 120_000; continue; }
		if (args[i] === "--help" || args[i] === "-h") {
			console.error("Usage: node skillbas-cli.mjs <file.sbas> [--stdin TEXT] [--resume JSON] [--max-instructions N] [--timeout MS]");
			process.exit(0);
		}
		if (!path) { path = args[i]; continue; }
	}

	if (!path) {
		console.error("Usage: node skillbas-cli.mjs <file.sbas> [--stdin TEXT] [--resume JSON] [--max-instructions N] [--timeout MS]");
		process.exit(1);
	}

	return { path, stdin, resume, maxInstructions, timeoutMs };
}

async function main() {
	const opts = parseArgs(process.argv);
	const source = await readFile(resolve(opts.path), "utf8");
	let resumeState = null;
	if (opts.resume) {
		try { resumeState = JSON.parse(opts.resume); }
		catch { console.error("Error: --resume is not valid JSON"); process.exit(1); }
	}

	const result = runSkillBasic(source, opts.stdin, opts.timeoutMs, opts.maxInstructions, resumeState);

	if (result.needsInput && result.resumeState) {
		console.log("---NEEDS INPUT---");
		console.log("inputPrompt: " + result.inputPrompt);
		console.log("stdout:\n" + result.stdout);
		console.log("prompts:\n" + result.prompts.map((p, i) => (i + 1) + ". " + p).join("\n"));
		console.log("resumeState: " + JSON.stringify(result.resumeState));
	} else {
		console.log("instructions: " + result.instructionCount);
		if (result.prompts.length) {
			console.log("prompts:\n" + result.prompts.map((p, i) => (i + 1) + ". " + p).join("\n"));
		} else {
			console.log("prompts: <none>");
		}
		if (result.stdout) console.log("stdout:\n" + result.stdout);
		if (result.stderr) console.log("stderr:\n" + result.stderr);
		if (result.timedOut) console.log("status: timed out");
		else if (result.stopped) console.log("status: stopped");
		else console.log("status: completed");
	}
}

main().catch((e) => { console.error(e); process.exit(1); });