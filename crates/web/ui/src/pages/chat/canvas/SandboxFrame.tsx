// ── Isolated iframe host ────────────────────────────────────
//
// The one way the web UI shows untrusted HTML (designs, MCP Apps, reply
// artifacts): a page on the separate render origin, framed with
// `sandbox="allow-scripts"` and nothing else, so it runs in an opaque origin
// with no cookies, storage or access to this page. See docs/src/canvas.md
// (including the WebKit frame-rate note).

import type { JSX } from "preact";
import { useEffect, useRef } from "preact/hooks";

/** Never add allow-same-origin: with a script it could remove its own sandbox. */
export const FRAME_SANDBOX = "allow-scripts";

interface SandboxFrameProps {
	src: string;
	title: string;
	className?: string;
	/** Dynamic size only (viewport width, reported height). */
	style?: JSX.CSSProperties;
	/** Messages whose `source` is this frame; parse them with the right protocol. */
	onMessage?: (event: MessageEvent) => void;
	onFrame?: (frame: HTMLIFrameElement | null) => void;
	onLoad?: () => void;
	lazy?: boolean;
	testId?: string;
}

export function SandboxFrame({
	src,
	title,
	className,
	style,
	onMessage,
	onFrame,
	onLoad,
	lazy = false,
	testId,
}: SandboxFrameProps) {
	const frameRef = useRef<HTMLIFrameElement>(null);
	const handlerRef = useRef(onMessage);
	handlerRef.current = onMessage;

	useEffect(() => {
		onFrame?.(frameRef.current);
		return () => onFrame?.(null);
	}, [onFrame]);

	useEffect(() => {
		const listener = (event: MessageEvent) => {
			const frame = frameRef.current;
			if (!frame || event.source !== frame.contentWindow) return;
			handlerRef.current?.(event);
		};
		window.addEventListener("message", listener);
		return () => window.removeEventListener("message", listener);
	}, []);

	return (
		<iframe
			ref={frameRef}
			src={src}
			title={title}
			sandbox={FRAME_SANDBOX}
			referrerpolicy="no-referrer"
			allow=""
			loading={lazy ? "lazy" : "eager"}
			className={className}
			style={style}
			onLoad={onLoad}
			data-testid={testId}
		/>
	);
}
