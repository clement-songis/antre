// ── Settings and account entry at the bottom of the sidebar ─
//
// Holds what used to crowd the top bar: settings, theme, the command
// palette, project links and sign-out, behind one menu.

import { render } from "preact";
import * as gon from "../gon";
import { t, useTranslation } from "../i18n";
import { navigate } from "../router";
import { routes } from "../routes";
import { togglePalette } from "../stores/command-store";
import { setThemeMode } from "../theme";
import { MenuButton, type MenuItem, type MenuSpec } from "./ContextMenu";

const LINKS: Array<{ id: string; labelKey: string; href: string; icon: string }> = [
	{
		id: "changelog",
		labelKey: "common:header.changelog",
		href: "https://www.moltis.org/changelog/",
		icon: "icon-document",
	},
	{
		id: "issues",
		labelKey: "common:header.reportIssueTitle",
		href: "https://github.com/moltis-org/moltis/issues/new/choose",
		icon: "icon-github-issue",
	},
	{
		id: "discussions",
		labelKey: "common:header.discussions",
		href: "https://github.com/moltis-org/moltis/discussions",
		icon: "icon-github-discussion",
	},
];

function accountMenu(): MenuSpec {
	const logoutBtn = document.getElementById("logoutBtn");
	const canSignOut = !!logoutBtn && logoutBtn.style.display !== "none";
	const items: MenuItem[] = [
		{
			id: "settings",
			label: t("common:header.settings"),
			icon: "icon-settings",
			onSelect: () => navigate(routes.profile as string),
		},
		{ id: "palette", label: t("common:header.commandPalette"), icon: "icon-search", onSelect: () => togglePalette() },
		{
			id: "theme-light",
			label: t("common:header.themeLight"),
			icon: "icon-sun",
			separatorBefore: true,
			onSelect: () => setThemeMode("light"),
		},
		{ id: "theme-dark", label: t("common:header.themeDark"), icon: "icon-moon", onSelect: () => setThemeMode("dark") },
		{
			id: "theme-system",
			label: t("common:header.themeSystem"),
			icon: "icon-monitor",
			onSelect: () => setThemeMode("system"),
		},
		...LINKS.map((link, index) => ({
			id: link.id,
			label: t(link.labelKey),
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
			label: t("common:header.signOut"),
			icon: "icon-logout",
			separatorBefore: true,
			onSelect: () => logoutBtn?.click(),
		});
	}
	return { label: t("chat:a11y.accountMenu"), items };
}

function initials(name: string): string {
	const parts = name.trim().split(/\s+/).filter(Boolean);
	const letters = parts.length > 1 ? `${parts[0][0]}${parts[parts.length - 1][0]}` : name.trim().slice(0, 2);
	return letters.toUpperCase() || "?";
}

export function SidebarAccount() {
	const { t: commonT } = useTranslation("common");
	const identity = gon.get("identity");
	const name = identity?.user_name || commonT("header.settings");
	return (
		<div className="sidebar-account flex items-center gap-2 px-1 py-1.5">
			<button
				type="button"
				className="sidebar-account-btn flex-1 min-w-0 flex items-center gap-2 bg-transparent border-0 cursor-pointer text-left p-1 rounded-corner-m"
				onClick={() => navigate(routes.profile as string)}
				title={commonT("header.settings")}
			>
				<span
					className="sidebar-account-avatar shrink-0 w-7 h-7 grid place-items-center rounded-corner-m bg-raised-2 border border-line font-display text-[11px] font-semibold text-ink-strong"
					aria-hidden="true"
				>
					{initials(name)}
				</span>
				<span className="truncate font-display text-label text-ink-strong">{name}</span>
			</button>
			<MenuButton label={commonT("chat:a11y.accountMenu")} getMenu={accountMenu} />
		</div>
	);
}

export function mountSidebarAccount(): void {
	const mount = document.getElementById("sidebarAccountMount");
	if (!mount) return;
	render(<SidebarAccount />, mount);
	document.body.classList.add("has-sidebar-account");
}
