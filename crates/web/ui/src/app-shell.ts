// ── App shell: sidebar collapse, phone drawer and top bar ────

import { closeSessions, isMobile, toggleSessions } from "./mobile";

const COLLAPSED_KEY = "antre-sidebar-collapsed";

function readCollapsed(): boolean {
	try {
		return localStorage.getItem(COLLAPSED_KEY) === "1";
	} catch {
		return false;
	}
}

function writeCollapsed(collapsed: boolean): void {
	try {
		localStorage.setItem(COLLAPSED_KEY, collapsed ? "1" : "0");
	} catch {
		// Private windows can refuse storage; the toggle still works for this page.
	}
}

export function setSidebarCollapsed(collapsed: boolean): void {
	document.body.classList.toggle("sidebar-collapsed", collapsed);
	const btn = document.getElementById("sidebarCollapseBtn");
	if (btn) {
		const label = collapsed ? "Expand sidebar" : "Collapse sidebar";
		btn.setAttribute("aria-label", label);
		btn.setAttribute("title", label);
		btn.setAttribute("aria-expanded", collapsed ? "false" : "true");
	}
	writeCollapsed(collapsed);
}

function syncMobileTitle(): void {
	const title = document.getElementById("mobileTitle");
	const name = document.getElementById("titleName");
	const emoji = document.getElementById("titleEmoji");
	if (!(title && name)) return;
	title.textContent = `${emoji?.textContent || ""}${name.textContent || ""}`.trim();
}

export function initAppShell(): void {
	setSidebarCollapsed(readCollapsed());

	document.getElementById("sidebarCollapseBtn")?.addEventListener("click", () => {
		setSidebarCollapsed(!document.body.classList.contains("sidebar-collapsed"));
	});

	document.getElementById("sidebarCloseBtn")?.addEventListener("click", () => closeSessions());

	document.getElementById("mobileMenuBtn")?.addEventListener("click", (e) => {
		e.stopPropagation();
		toggleSessions();
	});

	document.getElementById("mobileNewSessionBtn")?.addEventListener("click", () => {
		document.getElementById("newSessionBtn")?.click();
	});

	// Settings and sign-out live in the drawer on phones; leave it once used.
	for (const id of ["settingsBtn", "newSessionBtn", "commandPaletteBtn"]) {
		document.getElementById(id)?.addEventListener("click", () => {
			if (isMobile()) closeSessions();
		});
	}

	syncMobileTitle();
	const name = document.getElementById("titleName");
	if (name)
		new MutationObserver(syncMobileTitle).observe(name, { childList: true, characterData: true, subtree: true });

	document.addEventListener("keydown", (e) => {
		if (e.key === "b" && (e.metaKey || e.ctrlKey) && !e.shiftKey && !e.altKey && !isMobile()) {
			e.preventDefault();
			setSidebarCollapsed(!document.body.classList.contains("sidebar-collapsed"));
		}
	});
}
