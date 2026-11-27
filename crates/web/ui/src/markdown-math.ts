// ── Math in markdown ────────────────────────────────────────
//
// Marked extensions that keep TeX away from markdown parsing (underscores
// and asterisks are common in TeX) and emit placeholders. KaTeX renders the
// placeholders later, lazily, so pages without math never load it.
//
// Recognised: $$…$$ and \[…\] as display math, $…$ and \(…\) inline. A
// single `$` needs a non-space right after it and no digit right after the
// closing one, so prices like "$5 and $10" stay text.

import type { TokenizerAndRendererExtension, Tokens } from "marked";

interface MathToken extends Tokens.Generic {
	type: "mathBlock" | "mathInline";
	raw: string;
	tex: string;
}

function escapeHtml(text: string): string {
	return text.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
}

/** `tag` is "span" inside a paragraph even for display math, to keep the HTML valid. */
function placeholder(tex: string, display: boolean, tag: "div" | "span"): string {
	const cls = display ? "math-display" : "math-inline";
	return `<${tag} class="${cls}" data-tex="${escapeHtml(tex)}">${escapeHtml(tex)}</${tag}>`;
}

const BLOCK_RE = /^(?:\$\$([\s\S]+?)\$\$|\\\[([\s\S]+?)\\\])[ \t]*(?:\n|$)/;
const INLINE_RE = /^(?:\$\$([^\n]+?)\$\$|\\\(([^\n]+?)\\\)|\$(?=\S)([^$\n]*?\S)\$(?!\d))/;

export const mathBlockExtension: TokenizerAndRendererExtension = {
	name: "mathBlock",
	level: "block",
	start(src: string) {
		const index = src.search(/\$\$|\\\[/);
		return index < 0 ? undefined : index;
	},
	tokenizer(src: string): MathToken | undefined {
		const match = BLOCK_RE.exec(src);
		if (!match) return undefined;
		return { type: "mathBlock", raw: match[0], tex: (match[1] ?? match[2] ?? "").trim() };
	},
	renderer(token) {
		return `${placeholder((token as MathToken).tex, true, "div")}\n`;
	},
};

export const mathInlineExtension: TokenizerAndRendererExtension = {
	name: "mathInline",
	level: "inline",
	start(src: string) {
		const index = src.search(/\$|\\\(/);
		return index < 0 ? undefined : index;
	},
	tokenizer(src: string): MathToken | undefined {
		const match = INLINE_RE.exec(src);
		if (!match) return undefined;
		const display = match[1] !== undefined;
		return {
			type: "mathInline",
			raw: match[0],
			tex: (match[1] ?? match[2] ?? match[3] ?? "").trim(),
			display,
		} as MathToken;
	},
	renderer(token) {
		const t = token as MathToken & { display?: boolean };
		return placeholder(t.tex, t.display === true, "span");
	},
};
