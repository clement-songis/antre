// ── Copy buttons on rendered code blocks ────────────────────
//
// Markdown output is plain HTML (no component owns it), so one delegated
// listener serves every `.code-copy-btn` the renderer emits.

import { copyToClipboard } from "./ui";

const COPIED_RESET_MS = 1500;

function codeTextFor(button: HTMLElement): string | null {
	const code = button.closest("pre.code-block")?.querySelector("code");
	return code ? code.textContent || "" : null;
}

export function handleCodeCopyClick(e: MouseEvent): void {
	const target = e.target as HTMLElement | null;
	const button = target?.closest<HTMLButtonElement>(".code-copy-btn:not(.code-preview-btn)");
	if (!button) return;
	e.preventDefault();
	const text = codeTextFor(button);
	if (text === null) return;
	void copyToClipboard(text, "", "Could not copy — please copy manually.").then((ok) => {
		if (!ok) return;
		button.textContent = "Copied";
		button.classList.add("copied");
		setTimeout(() => {
			button.textContent = "Copy";
			button.classList.remove("copied");
		}, COPIED_RESET_MS);
	});
}

let installed = false;

export function installCodeCopyHandler(): void {
	if (installed) return;
	installed = true;
	document.addEventListener("click", handleCodeCopyClick);
}
