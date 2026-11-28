// ── Mermaid diagrams and KaTeX math in rendered markdown ────
//
// Both libraries are large, so they load on first use. Mermaid runs with
// securityLevel "strict" (its SVG output is sanitised, no click handlers)
// and KaTeX with trust off, so model output cannot inject markup.

type KatexModule = typeof import("katex");
type MermaidModule = typeof import("mermaid");

let katexPromise: Promise<KatexModule> | null = null;
let mermaidPromise: Promise<MermaidModule["default"]> | null = null;
let mermaidTheme: string | null = null;
let diagramCounter = 0;

function loadKatex(): Promise<KatexModule> {
	if (!katexPromise) {
		katexPromise = Promise.all([import("katex"), import("katex/dist/katex.min.css")]).then(([mod]) => mod);
	}
	return katexPromise;
}

function currentTheme(): "dark" | "default" {
	return document.documentElement.getAttribute("data-theme") === "dark" ? "dark" : "default";
}

async function loadMermaid(): Promise<MermaidModule["default"]> {
	if (!mermaidPromise) mermaidPromise = import("mermaid").then((mod) => mod.default);
	const mermaid = await mermaidPromise;
	const theme = currentTheme();
	if (mermaidTheme !== theme) {
		mermaid.initialize({ startOnLoad: false, securityLevel: "strict", theme });
		mermaidTheme = theme;
	}
	return mermaid;
}

async function renderMath(container: HTMLElement): Promise<void> {
	const nodes = container.querySelectorAll<HTMLElement>(".math-inline[data-tex], .math-display[data-tex]");
	if (nodes.length === 0) return;
	const katex = await loadKatex();
	for (const node of nodes) {
		if (node.dataset.rendered === "1") continue;
		katex.default.render(node.dataset.tex || "", node, {
			displayMode: node.classList.contains("math-display"),
			throwOnError: false,
			trust: false,
		});
		node.dataset.rendered = "1";
	}
}

async function renderDiagrams(container: HTMLElement): Promise<void> {
	const blocks = container.querySelectorAll<HTMLElement>('pre.code-block code[data-lang="mermaid"]');
	if (blocks.length === 0) return;
	const mermaid = await loadMermaid();
	for (const code of blocks) {
		const pre = code.closest("pre");
		if (!pre?.isConnected || pre.dataset.diagram === "1") continue;
		pre.dataset.diagram = "1";
		const source = code.textContent || "";
		try {
			diagramCounter += 1;
			const { svg } = await mermaid.render(`antre-mermaid-${diagramCounter}`, source);
			const figure = document.createElement("figure");
			figure.className = "mermaid-diagram";
			// Safe: mermaid's strict mode sanitises the SVG it returns.
			figure.innerHTML = svg; // eslint-disable-line no-unsanitized/property
			pre.before(figure);
			pre.classList.add("mermaid-source");
		} catch {
			// Invalid diagram: keep the source block as is.
		}
	}
}

/** Render math placeholders and mermaid fences inside `container`. */
export async function enhanceRichBlocks(container: HTMLElement | null): Promise<void> {
	if (!container) return;
	await Promise.all([renderMath(container), renderDiagrams(container)]);
}

/** Whether rendered markdown holds something `enhanceRichBlocks` would act on. */
export function hasRichBlocks(html: string): boolean {
	return html.includes('data-lang="mermaid"') || html.includes("data-tex=");
}
