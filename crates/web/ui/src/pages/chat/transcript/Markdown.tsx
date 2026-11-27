// ── Markdown rendering for transcript items ─────────────────
//
// `renderMarkdown` escapes every input before adding its own tags, so its
// output is safe to inject. A streaming reply is split into top-level
// blocks; memoised blocks keep their DOM, so only the growing last block is
// re-parsed and text selection in earlier blocks survives the stream.

import { memo } from "preact/compat";
import { useEffect, useMemo, useRef } from "preact/hooks";
import { highlightCodeBlocks } from "../../../code-highlight";
import { renderMarkdown, splitMarkdownBlocks } from "../../../helpers";
import { enhanceRichBlocks, hasRichBlocks } from "../../../rich-blocks";

interface BlockProps {
	raw: string;
	/** A closed block gets syntax highlighting; the open one would flicker. */
	closed: boolean;
}

const MarkdownBlock = memo(
	function MarkdownBlock({ raw, closed }: BlockProps) {
		const ref = useRef<HTMLDivElement>(null);
		const html = useMemo(() => renderMarkdown(raw), [raw]);
		useEffect(() => {
			if (!(closed && ref.current)) return;
			if (raw.includes("```")) void highlightCodeBlocks(ref.current);
			if (hasRichBlocks(html)) void enhanceRichBlocks(ref.current);
		}, [closed, html]);
		// biome-ignore lint/security/noDangerouslySetInnerHtml: renderMarkdown escapes its input first.
		return <div ref={ref} className="md-block contents" dangerouslySetInnerHTML={{ __html: html }} />;
	},
	(prev, next) => prev.raw === next.raw && prev.closed === next.closed,
);

interface MarkdownProps {
	text: string;
	streaming?: boolean;
}

export function Markdown({ text, streaming = false }: MarkdownProps) {
	const blocks = useMemo(() => splitMarkdownBlocks(text), [text]);
	return (
		<>
			{blocks.map((raw, index) => (
				<MarkdownBlock
					// Blocks only ever grow at the end, so the index is a stable key.
					// biome-ignore lint/suspicious/noArrayIndexKey: see above.
					key={index}
					raw={raw}
					closed={!streaming || index < blocks.length - 1}
				/>
			))}
		</>
	);
}
