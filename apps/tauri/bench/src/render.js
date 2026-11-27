// The chat's Markdown path, copied from crates/web/ui/src/helpers.ts
// (renderMarkdown) and ws/shared.ts (setSafeMarkdownHtml): same `marked`
// options and renderer. helpers.ts itself is not imported because it pulls
// the app's global state and i18n at import time.

import { Marked, Renderer } from "marked";

function esc(text) {
	return String(text)
		.replace(/&/g, "&amp;")
		.replace(/</g, "&lt;")
		.replace(/>/g, "&gt;")
		.replace(/"/g, "&quot;");
}

function sanitizeHref(href) {
	const trimmed = href.trim();
	return /^(https?:|mailto:|#)/i.test(trimmed) ? trimmed : "#";
}

const mdRenderer = new Renderer();
mdRenderer.code = ({ text, lang }) => {
	const langAttr = lang ? ` data-lang="${esc(lang)}"` : "";
	const badge = lang ? `<div class="code-lang-badge">${esc(lang)}</div>` : "";
	return `<pre class="code-block">${badge}<code${langAttr}>${esc(text)}</code></pre>\n`;
};
mdRenderer.table = function (token) {
	const renderCell = (cell) => this.parser.parseInline(cell.tokens);
	const header = token.header.map((cell) => `<th>${renderCell(cell)}</th>`).join("");
	const body = token.rows
		.map((row) => `<tr>${row.map((cell) => `<td>${renderCell(cell)}</td>`).join("")}</tr>`)
		.join("");
	return `<div class="msg-table-wrap"><table class="msg-table"><thead><tr>${header}</tr></thead><tbody>${body}</tbody></table></div>\n`;
};
mdRenderer.link = ({ href, text }) =>
	`<a href="${sanitizeHref(href)}" target="_blank" rel="noopener noreferrer">${text}</a>`;
mdRenderer.html = ({ text }) => esc(text);

const markedInstance = new Marked({ renderer: mdRenderer, breaks: true, gfm: true, async: false });

export function renderMarkdown(raw) {
	return markedInstance.parse(raw).trimEnd();
}

/** What the chat does on every streamed delta: re-render the whole text. */
export function setSafeMarkdownHtml(el, text) {
	const rendered = renderMarkdown(text);
	el.textContent = "";
	const wrapper = document.createElement("span");
	wrapper.insertAdjacentHTML("afterbegin", rendered);
	while (wrapper.firstChild) el.appendChild(wrapper.firstChild);
}
