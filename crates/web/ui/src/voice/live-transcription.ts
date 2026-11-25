// ── Live transcription client ────────────────────────────────
// Streams microphone audio to /api/stt/ws and reports partial and final
// transcripts. The protocol is documented in docs/src/voice.md.

const WORKLET_URL = "/assets/js/pcm-capture-worklet.js";
const READY_TIMEOUT_MS = 8000;
const DONE_TIMEOUT_MS = 25000;
const FLUSH_TIMEOUT_MS = 500;

interface ReadyMessage {
	type: "ready";
	provider: string;
	model: string;
	sampleRate: number;
}

interface SegmentMessage {
	type: "partial" | "final";
	segment: string;
	text: string;
}

interface DoneMessage {
	type: "done";
	text: string;
}

interface ErrorMessage {
	type: "error";
	code: string;
	message: string;
	fallback: boolean;
}

type ServerMessage = ReadyMessage | SegmentMessage | DoneMessage | ErrorMessage | { type: "speechStarted" };

export type LiveTranscriptionResult = { ok: true; text: string } | { ok: false; error: string; fallback: boolean };

export interface LiveTranscriptionHandlers {
	/** Called with the settled text and the pending (partial) text so far. */
	onText(settled: string, pending: string): void;
}

function parseMessage(raw: unknown): ServerMessage | null {
	if (typeof raw !== "string") return null;
	try {
		const value: unknown = JSON.parse(raw);
		if (value && typeof value === "object" && typeof (value as { type?: unknown }).type === "string") {
			return value as ServerMessage;
		}
	} catch {
		// Ignored: not a protocol message.
	}
	return null;
}

function socketUrl(): string {
	const scheme = window.location.protocol === "https:" ? "wss:" : "ws:";
	return `${scheme}//${window.location.host}/api/stt/ws`;
}

/**
 * One live transcription stream. Audio capture starts at once and is
 * buffered by the worklet until the provider is ready, so speech said while
 * connecting is not lost.
 */
export class LiveTranscription {
	private readonly socket: WebSocket;
	private context: AudioContext | null = null;
	private node: AudioWorkletNode | null = null;
	private source: MediaStreamAudioSourceNode | null = null;
	private readonly order: string[] = [];
	private readonly finals = new Map<string, string>();
	private readonly partials = new Map<string, string>();
	private settledResult: LiveTranscriptionResult | null = null;
	private resolveResult: ((result: LiveTranscriptionResult) => void) | null = null;
	private readonly result: Promise<LiveTranscriptionResult>;

	private constructor(
		private readonly handlers: LiveTranscriptionHandlers,
		socket: WebSocket,
	) {
		this.socket = socket;
		this.result = new Promise((resolve) => {
			this.resolveResult = resolve;
		});
	}

	/**
	 * Open a stream for `stream`. Resolves once the provider is ready, or
	 * with `null` when live transcription is unavailable (the caller then
	 * falls back to clip transcription).
	 */
	static async start(stream: MediaStream, handlers: LiveTranscriptionHandlers): Promise<LiveTranscription | null> {
		let socket: WebSocket;
		try {
			socket = new WebSocket(socketUrl());
		} catch (err) {
			console.warn("[voice] live transcription socket failed:", err);
			return null;
		}
		socket.binaryType = "arraybuffer";
		const live = new LiveTranscription(handlers, socket);
		const capture = live.startCapture(stream).catch((err: unknown) => {
			console.warn("[voice] live transcription capture failed:", err);
			return false;
		});
		const ready = await live.waitReady();
		if (!(ready && (await capture))) {
			live.cancel();
			return null;
		}
		live.node?.port.postMessage({ type: "rate", rate: ready.sampleRate });
		return live;
	}

	private async startCapture(stream: MediaStream): Promise<boolean> {
		this.context = new AudioContext();
		await this.context.audioWorklet.addModule(WORKLET_URL);
		this.node = new AudioWorkletNode(this.context, "pcm-capture", { numberOfOutputs: 0 });
		this.node.port.onmessage = (event: MessageEvent): void => {
			if (event.data instanceof ArrayBuffer && this.socket.readyState === WebSocket.OPEN) {
				this.socket.send(event.data);
			}
		};
		this.source = this.context.createMediaStreamSource(stream);
		this.source.connect(this.node);
		return true;
	}

	private waitReady(): Promise<ReadyMessage | null> {
		return new Promise((resolve) => {
			const timer = setTimeout(() => resolve(null), READY_TIMEOUT_MS);
			const finish = (value: ReadyMessage | null): void => {
				clearTimeout(timer);
				resolve(value);
			};
			this.socket.onopen = (): void => {
				this.socket.send(JSON.stringify({ type: "start" }));
			};
			this.socket.onerror = (): void => finish(null);
			this.socket.onclose = (): void => {
				finish(null);
				this.settle({ ok: false, error: "connection closed", fallback: true });
			};
			this.socket.onmessage = (event: MessageEvent): void => {
				const message = parseMessage(event.data);
				if (message?.type === "ready") {
					finish(message);
				} else if (message?.type === "error") {
					console.warn("[voice] live transcription unavailable:", message.message);
					finish(null);
				}
				if (message) this.onMessage(message);
			};
		});
	}

	private onMessage(message: ServerMessage): void {
		switch (message.type) {
			case "partial":
				this.track(message.segment);
				this.partials.set(message.segment, message.text);
				this.emitText();
				break;
			case "final":
				this.track(message.segment);
				this.partials.delete(message.segment);
				this.finals.set(message.segment, message.text.trim());
				this.emitText();
				break;
			case "done":
				this.settle({ ok: true, text: message.text.trim() });
				break;
			case "error":
				this.settle({ ok: false, error: message.message, fallback: message.fallback });
				break;
			default:
				break;
		}
	}

	private track(segment: string): void {
		if (!this.order.includes(segment)) this.order.push(segment);
	}

	private emitText(): void {
		const settled: string[] = [];
		const pending: string[] = [];
		for (const segment of this.order) {
			const final = this.finals.get(segment);
			if (final !== undefined) {
				if (final) settled.push(final);
				continue;
			}
			const partial = this.partials.get(segment)?.trim();
			if (partial) pending.push(partial);
		}
		this.handlers.onText(settled.join(" "), pending.join(" "));
	}

	private settle(result: LiveTranscriptionResult): void {
		if (this.settledResult) return;
		this.settledResult = result;
		this.resolveResult?.(result);
		this.releaseAudio();
		if (this.socket.readyState === WebSocket.OPEN || this.socket.readyState === WebSocket.CONNECTING) {
			this.socket.close();
		}
	}

	private releaseAudio(): void {
		this.source?.disconnect();
		this.source = null;
		if (this.node) this.node.port.onmessage = null;
		this.node = null;
		this.context?.close().catch(() => undefined);
		this.context = null;
	}

	private flushAudio(): Promise<void> {
		const node = this.node;
		if (!node) return Promise.resolve();
		return new Promise((resolve) => {
			const timer = setTimeout(resolve, FLUSH_TIMEOUT_MS);
			const forward = node.port.onmessage;
			node.port.onmessage = (event: MessageEvent): void => {
				if (event.data?.type === "flushed") {
					clearTimeout(timer);
					resolve();
					return;
				}
				forward?.call(node.port, event);
			};
			node.port.postMessage({ type: "flush" });
		});
	}

	/** End the audio and wait for the final transcript. */
	async stop(): Promise<LiveTranscriptionResult> {
		if (this.settledResult) return this.settledResult;
		this.source?.disconnect();
		await this.flushAudio();
		if (this.socket.readyState === WebSocket.OPEN) {
			this.socket.send(JSON.stringify({ type: "stop" }));
		}
		const timeout = new Promise<LiveTranscriptionResult>((resolve) => {
			setTimeout(() => resolve({ ok: false, error: "live transcription timed out", fallback: true }), DONE_TIMEOUT_MS);
		});
		const result = await Promise.race([this.result, timeout]);
		this.settle(result);
		return result;
	}

	/** Drop the stream without a transcript. */
	cancel(): void {
		if (this.socket.readyState === WebSocket.OPEN) {
			this.socket.send(JSON.stringify({ type: "cancel" }));
		}
		this.settle({ ok: false, error: "cancelled", fallback: false });
	}
}
