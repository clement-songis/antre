// ── Text sent to the agent for picked elements and comment pins ───

import type { Design, DesignComment, ElementContext, PickedElement } from "./types";

function oneLine(text: string, max: number): string {
	const flat = text.replace(/\s+/g, " ").trim();
	return flat.length > max ? `${flat.slice(0, max - 1)}…` : flat;
}

/** How an element is named for the agent: its `data-mid`, else its selector. */
export function elementLabel(element: Pick<PickedElement, "mid" | "selector">): string {
	return element.mid ? `[data-mid="${element.mid}"]` : element.selector;
}

function elementLine(element: PickedElement): string {
	const quoted = element.text ? ` "${oneLine(element.text, 120)}"` : "";
	return `\`${elementLabel(element)}\` <${element.tag}>${quoted}`;
}

function designHeading(title: string, designId: string, version: number | null): string {
	const at = version ? `, version ${version}` : "";
	return `design "${oneLine(title, 80)}" (design_id: ${designId}${at})`;
}

/** The composer text followed by the picked elements, grouped by design. */
export function withElementContext(text: string, contexts: ElementContext[]): string {
	if (contexts.length === 0) return text;
	const groups = new Map<string, ElementContext[]>();
	for (const context of contexts) {
		const key = `${context.designId}@${context.version ?? "live"}`;
		groups.set(key, [...(groups.get(key) ?? []), context]);
	}
	const sections = [...groups.values()].map((group) => {
		const first = group[0];
		const lines = group.map((context) => `- ${elementLine(context.element)}`);
		return [`Selected in ${designHeading(first.designTitle, first.designId, first.version)}:`, ...lines].join("\n");
	});
	return [text, ...sections].filter(Boolean).join("\n\n");
}

/** One prompt asking the agent to address every open comment pin. */
export function commentsPrompt(design: Design, version: number, pins: DesignComment[]): string {
	const lines = pins.map((pin, index) => {
		const anchor = pin.mid
			? `\`[data-mid="${pin.mid}"]\``
			: pin.selector
				? `\`${pin.selector}\``
				: `the point (${pin.x ?? 0}, ${pin.y ?? 0})`;
		const context = pin.elementText ? ` ("${oneLine(pin.elementText, 80)}")` : "";
		return `${index + 1}. On ${anchor}${context}: ${pin.body.trim()}`;
	});
	return [`Please address these comments on ${designHeading(design.title, design.id, version)}:`, ...lines].join("\n");
}
