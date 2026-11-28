const root = document.documentElement;

document.querySelector("[data-theme-toggle]")?.addEventListener("click", () => {
	const next = root.dataset.theme === "dark" ? "light" : "dark";
	root.dataset.theme = next;
	try {
		localStorage.setItem("antre-site-theme", next);
	} catch {}
});

async function copyText(text) {
	try {
		await navigator.clipboard.writeText(text);
		return true;
	} catch {
		return false;
	}
}

for (const tabs of document.querySelectorAll("[data-tabs]")) {
	const buttons = [...tabs.querySelectorAll('[role="tab"]')];
	const panels = buttons.map((button) => document.getElementById(button.getAttribute("aria-controls")));

	const select = (index, focus) => {
		buttons.forEach((button, i) => {
			const active = i === index;
			button.setAttribute("aria-selected", String(active));
			button.tabIndex = active ? 0 : -1;
			panels[i].hidden = !active;
		});
		if (focus) buttons[index].focus();
	};

	buttons.forEach((button, index) => {
		button.addEventListener("click", () => select(index, false));
		button.addEventListener("keydown", (event) => {
			const moves = { ArrowRight: 1, ArrowLeft: -1 };
			if (event.key in moves) {
				event.preventDefault();
				select((index + moves[event.key] + buttons.length) % buttons.length, true);
			} else if (event.key === "Home") {
				event.preventDefault();
				select(0, true);
			} else if (event.key === "End") {
				event.preventDefault();
				select(buttons.length - 1, true);
			}
		});
	});

	const copyLabel = tabs.dataset.copyLabel || "Copy";
	const copiedLabel = tabs.dataset.copiedLabel || "Copied";
	for (const pre of tabs.querySelectorAll("pre")) {
		const button = document.createElement("button");
		button.type = "button";
		button.className = "copy";
		button.textContent = copyLabel;
		button.addEventListener("click", async () => {
			const ok = await copyText(pre.querySelector("code").textContent);
			button.textContent = ok ? copiedLabel : copyLabel;
			setTimeout(() => {
				button.textContent = copyLabel;
			}, 1400);
		});
		pre.appendChild(button);
	}

	tabs.classList.add("is-ready");
	select(0, false);
}
