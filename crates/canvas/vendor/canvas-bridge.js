/* Preview bridge, protocol "antre.canvas/1". Injected by the render origin
 * into previewed designs; see docs/src/canvas.md for the message contract. */
(() => {
	const PROTOCOL = "antre.canvas/1";
	const MAX_MIDS = 500;
	const BRIDGE_ATTR = "data-antre-bridge";
	if (window.parent === window) return;

	const hostMatch = /(?:^#|&)host=([^&]*)/.exec(location.hash);
	let hostOrigin = null;
	try {
		hostOrigin = hostMatch ? decodeURIComponent(hostMatch[1]) : null;
	} catch (_error) {
		hostOrigin = null;
	}
	if (!(hostOrigin && /^https?:\/\/[^/?#\s]+$/.test(hostOrigin))) return;

	const cap = (value, max) => {
		const text = String(value ?? "");
		return text.length > max ? `${text.slice(0, max - 1)}…` : text;
	};
	const send = (type, fields) => {
		window.parent.postMessage({ protocol: PROTOCOL, type, ...fields }, hostOrigin);
	};

	const mids = () => {
		const seen = new Set();
		for (const node of document.querySelectorAll("[data-mid]")) {
			if (seen.size >= MAX_MIDS) break;
			seen.add(cap(node.getAttribute("data-mid"), 120));
		}
		return [...seen];
	};

	const isBridgeNode = (node) => node instanceof Element && node.closest(`[${BRIDGE_ATTR}]`) !== null;

	const cssEscape = (value) => (window.CSS?.escape ? CSS.escape(value) : value.replace(/[^\w-]/g, "\\$&"));

	const selectorFor = (element) => {
		const parts = [];
		let node = element;
		while (node && node.nodeType === 1 && node !== document.documentElement && parts.length < 6) {
			if (node.id) {
				parts.unshift(`#${cssEscape(node.id)}`);
				break;
			}
			let part = node.tagName.toLowerCase();
			const parent = node.parentElement;
			if (parent) {
				const same = [...parent.children].filter((child) => child.tagName === node.tagName);
				if (same.length > 1) part += `:nth-of-type(${same.indexOf(node) + 1})`;
			}
			parts.unshift(part);
			node = parent;
		}
		return cap(parts.join(" > "), 500);
	};

	const findTarget = (fields) => {
		if (typeof fields.mid === "string" && fields.mid) {
			const byMid = document.querySelector(`[data-mid="${cssEscape(fields.mid)}"]`);
			if (byMid) return byMid;
		}
		if (typeof fields.selector === "string" && fields.selector) {
			try {
				return document.querySelector(fields.selector);
			} catch (_error) {
				return null;
			}
		}
		return null;
	};

	const describe = (element) => {
		const rect = element.getBoundingClientRect();
		const mid = element.getAttribute("data-mid");
		return {
			...(mid ? { mid: cap(mid, 120) } : {}),
			selector: selectorFor(element),
			tag: element.tagName.toLowerCase(),
			text: cap((element.innerText || element.textContent || "").replace(/\s+/g, " ").trim(), 500),
			html: cap(element.outerHTML, 300),
			rect: {
				x: Math.round(rect.left + window.scrollX),
				y: Math.round(rect.top + window.scrollY),
				width: Math.round(rect.width),
				height: Math.round(rect.height),
			},
		};
	};

	const makeBox = (color, dashed) => {
		const box = document.createElement("div");
		box.setAttribute(BRIDGE_ATTR, "");
		box.style.cssText =
			"position:absolute;pointer-events:none;z-index:2147483646;box-sizing:border-box;" +
			`border:2px ${dashed ? "dashed" : "solid"} ${color};border-radius:3px;display:none;`;
		return box;
	};
	const place = (box, element) => {
		if (!element) {
			box.style.display = "none";
			return;
		}
		const rect = element.getBoundingClientRect();
		box.style.display = "block";
		box.style.left = `${rect.left + window.scrollX - 2}px`;
		box.style.top = `${rect.top + window.scrollY - 2}px`;
		box.style.width = `${rect.width + 4}px`;
		box.style.height = `${rect.height + 4}px`;
	};

	const hoverBox = makeBox("#1a8a99", false);
	const highlightBox = makeBox("#e9b04f", true);
	let picking = false;
	let rawPick = false;
	let hovered = null;
	let highlighted = null;
	let pins = [];
	const pinLayer = document.createElement("div");
	pinLayer.setAttribute(BRIDGE_ATTR, "");
	pinLayer.style.cssText = "position:absolute;left:0;top:0;width:0;height:0;z-index:2147483647;";

	const pickTarget = (node) => {
		if (!(node instanceof Element) || isBridgeNode(node)) return null;
		if (rawPick) return node;
		return node.closest("[data-mid]") || node;
	};

	const stopPicking = () => {
		picking = false;
		hovered = null;
		place(hoverBox, null);
		document.documentElement.style.cursor = "";
	};

	const onMove = (event) => {
		if (!picking) return;
		rawPick = event.altKey;
		hovered = pickTarget(event.target);
		place(hoverBox, hovered);
	};
	const onClick = (event) => {
		if (!picking) return;
		event.preventDefault();
		event.stopPropagation();
		const target = pickTarget(event.target);
		stopPicking();
		if (target) send("pick.result", { element: describe(target) });
		else send("pick.cancel", {});
	};
	const onKey = (event) => {
		if (picking && event.key === "Escape") {
			event.preventDefault();
			stopPicking();
			send("pick.cancel", {});
		}
	};

	const renderPins = () => {
		pinLayer.replaceChildren();
		for (const pin of pins) {
			const target = findTarget(pin);
			let left = typeof pin.x === "number" ? pin.x : null;
			let top = typeof pin.y === "number" ? pin.y : null;
			if (target) {
				const rect = target.getBoundingClientRect();
				left = rect.right + window.scrollX - 10;
				top = rect.top + window.scrollY - 10;
			}
			if (left === null || top === null) continue;
			const badge = document.createElement("button");
			badge.type = "button";
			badge.textContent = String(pin.n);
			badge.setAttribute("aria-label", `Comment ${pin.n}`);
			badge.style.cssText =
				"position:absolute;width:22px;height:22px;border-radius:11px 11px 11px 3px;border:0;" +
				"background:#1a8a99;color:#fff;font:600 11px/22px system-ui,sans-serif;text-align:center;" +
				`padding:0;cursor:pointer;box-shadow:0 1px 4px rgba(0,0,0,.3);left:${Math.max(0, left)}px;top:${Math.max(0, top)}px;`;
			badge.addEventListener("click", (event) => {
				event.preventDefault();
				event.stopPropagation();
				send("pin.open", { id: pin.id });
			});
			pinLayer.appendChild(badge);
		}
	};

	const reposition = () => {
		if (highlighted) place(highlightBox, highlighted);
		if (pins.length) renderPins();
	};

	const validPins = (value) =>
		Array.isArray(value)
			? value
					.filter((pin) => pin && typeof pin.id === "string" && typeof pin.n === "number")
					.slice(0, 200)
					.map((pin) => ({
						id: cap(pin.id, 64),
						n: Math.trunc(pin.n),
						mid: typeof pin.mid === "string" ? pin.mid : undefined,
						selector: typeof pin.selector === "string" ? cap(pin.selector, 500) : undefined,
						x: typeof pin.x === "number" ? pin.x : undefined,
						y: typeof pin.y === "number" ? pin.y : undefined,
					}))
			: [];

	window.addEventListener("message", (event) => {
		if (event.source !== window.parent || event.origin !== hostOrigin) return;
		const data = event.data;
		if (!data || typeof data !== "object" || data.protocol !== PROTOCOL) return;
		switch (data.type) {
			case "pick.start":
				picking = true;
				document.documentElement.style.cursor = "crosshair";
				break;
			case "pick.stop":
				stopPicking();
				break;
			case "highlight":
				highlighted = findTarget(data);
				place(highlightBox, highlighted);
				highlighted?.scrollIntoView({ block: "nearest", behavior: "smooth" });
				break;
			case "pins":
				pins = validPins(data.pins);
				renderPins();
				break;
			default:
				break;
		}
	});

	window.addEventListener("error", (event) => {
		send("error", {
			message: cap(event.message || "Script error", 1000),
			...(event.filename ? { source: cap(event.filename.split("/").pop(), 200) } : {}),
			...(event.lineno ? { line: event.lineno } : {}),
		});
	});
	window.addEventListener("unhandledrejection", (event) => {
		send("error", { message: cap(`Unhandled rejection: ${event.reason}`, 1000) });
	});

	const start = () => {
		document.body.append(hoverBox, highlightBox, pinLayer);
		document.addEventListener("mousemove", onMove, true);
		document.addEventListener("click", onClick, true);
		document.addEventListener("keydown", onKey, true);
		window.addEventListener("scroll", reposition, { passive: true });
		window.addEventListener("resize", reposition);
		let lastHeight = -1;
		const reportSize = () => {
			const height = Math.ceil(document.documentElement.scrollHeight);
			if (height === lastHeight) return;
			lastHeight = height;
			send("resize", { height });
			reposition();
		};
		if (window.ResizeObserver) new ResizeObserver(reportSize).observe(document.documentElement);
		send("ready", { title: cap(document.title, 200), mids: mids() });
		reportSize();
	};
	if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", start, { once: true });
	else start();
})();
