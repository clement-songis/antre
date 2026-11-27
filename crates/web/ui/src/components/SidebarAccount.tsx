// ── Settings and account entry at the bottom of the sidebar ─
//
// Holds what used to crowd the top bar: settings, theme, the command
// palette, project links and sign-out, behind one menu.

import { render } from "preact";
import * as gon from "../gon";
import { navigate } from "../router";
import { routes } from "../routes";
import { togglePalette } from "../stores/command-store";
import { setThemeMode } from "../theme";
import { MenuButton, type MenuItem, type MenuSpec } from "./ContextMenu";

const LINKS: Array<{ id: string; label: string; href: string; icon: string }> = [
	{ id: "changelog", label: "Changelog", href: "https://www.moltis.org/changelog/", icon: "icon-document" },
	{
		id: "issues",
		label: "Report an issue",
		href: "https://github.com/moltis-org/moltis/issues/new/choose",
		icon: "icon-github-issue",
	},
	{
		id: "discussions",
		label: "Discussions",
		href: "https://github.com/moltis-org/moltis/discussions",
		icon: "icon-github-discussion",
	},
];

function accountMenu(): MenuSpec {
	const logoutBtn = document.getElementById("logoutBtn");
	const canSignOut = !!logoutBtn && logoutBtn.style.display !== "none";
	const items: MenuItem[] = [
		{ id: "settings", label: "Settings", icon: "icon-settings", onSelect: () => navigate(routes.profile as string) },
		{ id: "palette", label: "Command palette", icon: "icon-search", onSelect: () => togglePalette() },
		{
			id: "theme-light",
			label: "Light theme",
			icon: "icon-sun",
			separatorBefore: true,
			onSelect: () => setThemeMode("light"),
		},
		{ id: "theme-dark", label: "Dark theme", icon: "icon-moon", onSelect: () => setThemeMode("dark") },
		{ id: "theme-system", label: "System theme", icon: "icon-monitor", onSelect: () => setThemeMode("system") },
		...LINKS.map((link, index) => ({
			id: link.id,
			label: link.label,
			icon: link.icon,
			separatorBefore: index === 0,
			onSelect: () => {
				window.open(link.href, "_blank", "noopener,noreferrer");
			},
		})),
	];
	if (canSignOut) {
		items.push({
			id: "sign-out",
			label: "Sign out",
			icon: "icon-logout",
			separatorBefore: true,
			onSelect: () => logoutBtn?.click(),
		});
	}
	return { label: "Settings and account", items };
}

export function SidebarAccount() {
	const identity = gon.get("identity");
	const name = identity?.user_name || "Settings";
	return (
		<div className="sidebar-account flex items-center gap-2 px-3 py-2">
			<button
				type="button"
				className="sidebar-account-btn flex-1 min-w-0 flex items-center gap-2 text-xs text-[var(--text)] bg-transparent border-0 cursor-pointer text-left p-1 rounded"
				onClick={() => navigate(routes.profile as string)}
				title="Settings"
			>
				<span className="icon icon-sm icon-settings shrink-0" aria-hidden="true" />
				<span className="truncate">{name}</span>
			</button>
			<MenuButton label="Settings and account" getMenu={accountMenu} />
		</div>
	);
}

export function mountSidebarAccount(): void {
	const mount = document.getElementById("sidebarAccountMount");
	if (!mount) return;
	render(<SidebarAccount />, mount);
	document.body.classList.add("has-sidebar-account");
}
