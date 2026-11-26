// AudioWorklet for live transcription: mixes the microphone to mono,
// resamples it to the provider rate and posts 100 ms PCM16 frames.
//
// Loaded from a static file because the app CSP forbids blob: scripts.
// Audio captured before the provider rate is known is kept and resampled
// once the main thread sends it, so nothing said while connecting is lost.

const MAX_PENDING_SECONDS = 15;

class PcmCaptureProcessor extends AudioWorkletProcessor {
	constructor() {
		super();
		this.targetRate = 0;
		this.ratio = 1;
		this.pending = [];
		this.pendingSamples = 0;
		this.position = 0;
		this.last = 0;
		this.frame = null;
		this.filled = 0;
		this.port.onmessage = (event) => this.onMessage(event.data);
	}

	onMessage(message) {
		if (message?.type === "rate") {
			this.targetRate = message.rate;
			this.ratio = sampleRate / message.rate;
			this.frame = new Int16Array(Math.round(message.rate / 10));
			for (const block of this.pending) this.resample(block);
			this.pending = [];
			this.pendingSamples = 0;
		} else if (message?.type === "flush") {
			this.postFrame(this.filled);
			this.port.postMessage({ type: "flushed" });
		}
	}

	process(inputs) {
		const channels = inputs[0];
		if (!channels || channels.length === 0) return true;
		const length = channels[0].length;
		const mono = new Float32Array(length);
		for (const channel of channels) {
			for (let i = 0; i < length; i++) mono[i] += channel[i] / channels.length;
		}
		if (this.targetRate) {
			this.resample(mono);
		} else if (this.pendingSamples < MAX_PENDING_SECONDS * sampleRate) {
			this.pending.push(mono);
			this.pendingSamples += length;
		}
		return true;
	}

	// Linear interpolation; `position` is relative to the current block, and
	// index -1 is the last sample of the previous block.
	resample(block) {
		const length = block.length;
		while (this.position < length - 1) {
			const index = Math.floor(this.position);
			const frac = this.position - index;
			const a = index < 0 ? this.last : block[index];
			const b = block[index + 1];
			const sample = Math.max(-1, Math.min(1, a + (b - a) * frac));
			this.frame[this.filled++] = sample < 0 ? sample * 0x8000 : sample * 0x7fff;
			if (this.filled === this.frame.length) this.postFrame(this.filled);
			this.position += this.ratio;
		}
		this.position -= length;
		this.last = block[length - 1];
	}

	postFrame(samples) {
		if (!this.frame || samples === 0) return;
		const out = this.frame.slice(0, samples);
		this.port.postMessage(out.buffer, [out.buffer]);
		this.filled = 0;
	}
}

registerProcessor("pcm-capture", PcmCaptureProcessor);
