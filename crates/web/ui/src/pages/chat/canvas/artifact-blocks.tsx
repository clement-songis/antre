// ── Inline previews of HTML code blocks in replies ──────────
//
// A "Preview" button on ```html blocks stages the snippet on the render
// origin and shows it below the block in the shared sandboxed frame.

import { render } from "preact";
import { useState } from "preact/hooks";
import { t } from "../../../i18n";
import { stageFrame } from "./api";
import { parseFrameMessage, withHostHash } from "./bridge";
import { SandboxFrame } from "./SandboxFrame";

const PREVIEWABLE = new Set(["html", "htm", "svg"]);
const MIN_HEIGHT = 80;
const MAX_HEIGHT = 1200;

function ArtifactFrame({ url }: { url: string }) {
	const [height, setHeight] = useState(320);
	return (
		<SandboxFrame
			src={withHostHash(url)}
			title={t("canvas:artifactTitle")}
			className="inline-frame mt-2"
			style={{ height: `${height}px` }}
			lazy={true}
			testId="artifact-frame"
			onMessage={(event) => {
				const message = parseFrameMessage(event.data);
				if (message?.type === "resize") {
					setHeight(Math.min(Math.max(Math.ceil(message.height), MIN_HEIGHT), MAX_HEIGHT));
				}
			}}
		/>
	);
}

async function togglePreview(pre: HTMLElement, code: HTMLElement, button: HTMLButtonElement): Promise<void> {
	const existing = pre.nextElementSibling;
	if (existing instanceof HTMLElement && existing.dataset.artifactPreview === "1") {
		render(null, existing);
		existing.remove();
		button.textContent = t("canvas:artifactPreview");
		button.setAttribute("aria-expanded", "false");
		return;
	}
	button.disabled = true;
	try {
		const lang = code.dataset.lang?.toLowerCase();
		const source = code.textContent || "";
		const html = lang === "svg" ? `<!doctype html><body style="margin:0">${source}</body>` : source;
		const link = await stageFrame("artifact", html);
		const host = document.createElement("div");
		host.dataset.artifactPreview = "1";
		pre.after(host);
		render(<ArtifactFrame url={link.url} />, host);
		button.textContent = t("canvas:artifactHide");
		button.setAttribute("aria-expanded", "true");
	} catch (error) {
		button.title = error instanceof Error ? error.message : String(error);
	} finally {
		button.disabled = false;
	}
}

/** Add preview buttons to the HTML/SVG code blocks inside `container`. */
export function enhanceArtifactBlocks(container: HTMLElement | null): void {
	if (!container) return;
	for (const code of container.querySelectorAll<HTMLElement>("pre.code-block code[data-lang]")) {
		if (!PREVIEWABLE.has(code.dataset.lang?.toLowerCase() ?? "")) continue;
		const pre = code.closest("pre");
		const tools = pre?.querySelector(".code-block-tools");
		if (!(pre && tools) || tools.querySelector(".code-preview-btn")) continue;
		const button = document.createElement("button");
		button.type = "button";
		button.className = "code-copy-btn code-preview-btn";
		button.textContent = t("canvas:artifactPreview");
		button.setAttribute("aria-expanded", "false");
		button.addEventListener("click", (event) => {
			event.stopPropagation();
			void togglePreview(pre, code, button);
		});
		tools.prepend(button);
	}
}

export function hasArtifactBlocks(html: string): boolean {
	return /data-lang="(html|htm|svg)"/i.test(html);
}
