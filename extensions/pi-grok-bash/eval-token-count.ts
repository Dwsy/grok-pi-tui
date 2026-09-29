/**
 * Exact text token accounting via the npm "tokenizers" Rust binding.
 *
 * Token counts are meaningful only for the configured tokenizer.json. Do not
 * substitute a character heuristic, pretend these are provider-billed tokens,
 * or count image base64 as textual model tokens.
 */
import { createRequire } from "node:module";
import { homedir } from "node:os";
import path from "node:path";

export type TokenUsage = {
	engine: "tokenizers";
	status: "ready" | "unavailable";
	tokenizer_file: string | null;
	input_tokens: number | null;
	output_tokens: number | null;
	total_tokens: number | null;
	note: string;
};

type Encoding = { getIds?: () => ArrayLike<number>; getTokens?: () => ArrayLike<string>; ids?: ArrayLike<number> };
type Tokenizer = { encode: (text: string, pair?: string | null, addSpecialTokens?: boolean) => Encoding | Promise<Encoding> };
type TokenizerPackage = { Tokenizer?: { fromFile: (filename: string) => Tokenizer | Promise<Tokenizer> } };

function installedTokenizers(): TokenizerPackage | undefined {
	const roots = [
		process.env.PI_PACKAGE_DIR && path.join(process.env.PI_PACKAGE_DIR, "package.json"),
		path.join(homedir(), ".pi", "agent", "npm", "package.json"),
		path.join(process.cwd(), "package.json"),
	].filter((value): value is string => Boolean(value));
	for (const root of roots) {
		try {
			const mod = createRequire(root)("tokenizers") as TokenizerPackage;
			if (typeof mod.Tokenizer?.fromFile === "function") return mod;
		} catch {
			// Resolve against the next Pi runtime root.
		}
	}
	return undefined;
}

export class EvalTokenCounter {
	private constructor(
		private readonly tokenizer: Tokenizer | undefined,
		readonly file: string | null,
		readonly error: string | undefined,
	) {}

	static async create(configuredFile = process.env.PI_GROK_EVAL_MCP_TOKENIZER_FILE?.trim()
		|| path.join(process.env.GROK_HOME?.trim() || path.join(homedir(), ".grok-pi"), "eval-pi-mcp", "tokenizer.json"),
		loader: (() => TokenizerPackage | undefined) = installedTokenizers): Promise<EvalTokenCounter> {
		const mod = loader();
		if (!mod?.Tokenizer) {
			return new EvalTokenCounter(undefined, configuredFile,
				'Install npm package "tokenizers" into Pi\'s npm tree (npm install --prefix ~/.pi/agent/npm tokenizers).');
		}
		try {
			return new EvalTokenCounter(await Promise.resolve(mod.Tokenizer.fromFile(configuredFile)), configuredFile, undefined);
		} catch (error) {
			return new EvalTokenCounter(undefined, configuredFile,
				`Cannot load tokenizer.json: ${error instanceof Error ? error.message : String(error)}`);
		}
	}

	get ready(): boolean { return this.tokenizer !== undefined; }

	private async tokens(text: string): Promise<number> {
		const encoded = await Promise.resolve(this.tokenizer!.encode(text, null, false));
		const ids = encoded.getIds?.() ?? encoded.ids ?? encoded.getTokens?.();
		if (!ids || !Number.isInteger(ids.length)) throw new Error('Unsupported "tokenizers" Encoding API');
		return ids.length;
	}

	/** Counts the MCP tool's input/output text payload excluding the accounting footer.
	 *  Image resources and the bearer key are never submitted to the tokenizer.
	 */
	async count(input: string, output: string): Promise<TokenUsage> {
		if (!this.tokenizer) {
			return {
				engine: "tokenizers", status: "unavailable", tokenizer_file: this.file,
				input_tokens: null, output_tokens: null, total_tokens: null,
				note: this.error ?? "Tokenizer not initialized",
			};
		}
		try {
			const inputTokens = await this.tokens(input);
			const outputTokens = await this.tokens(output);
			return {
				engine: "tokenizers", status: "ready", tokenizer_file: this.file,
				input_tokens: inputTokens, output_tokens: outputTokens,
				total_tokens: inputTokens + outputTokens,
				note: "Text payload only, using the selected tokenizer.json; not provider usage or image-token billing.",
			};
		} catch (error) {
			return {
				engine: "tokenizers", status: "unavailable", tokenizer_file: this.file,
				input_tokens: null, output_tokens: null, total_tokens: null,
				note: `Tokenizer encode error: ${error instanceof Error ? error.message : String(error)}`,
			};
		}
	}
}
