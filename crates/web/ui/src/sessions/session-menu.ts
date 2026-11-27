// ── Session context menu ────────────────────────────────────

import { type MenuItem, type MenuSpec, openContextMenu } from "../components/ContextMenu";
import { projectStore } from "../stores/project-store";
import type { Session } from "../stores/session-store";
import {
	canRenameSession,
	deleteSessionWithUndo,
	forkSession,
	moveSessionToProject,
	pinnedSessionKeys,
	renamingSessionKey,
	setArchived,
	shareSession,
	togglePinned,
} from "./session-actions";

function projectMenu(session: Session, anchor: { x: number; y: number }): void {
	const items: MenuItem[] = projectStore.projects.value.map((project) => ({
		id: `project-${project.id}`,
		label: String(project.label || project.id),
		icon: project.id === session.projectId ? "icon-checkmark" : "icon-folder",
		onSelect: () => void moveSessionToProject(session.key, project.id),
	}));
	items.push({
		id: "project-none",
		label: "No project",
		separatorBefore: items.length > 0,
		disabled: !session.projectId,
		onSelect: () => void moveSessionToProject(session.key, ""),
	});
	openContextMenu({ label: "Move to project", items }, anchor);
}

export function sessionMenu(session: Session): MenuSpec {
	const pinned = pinnedSessionKeys.value.has(session.key);
	const isCron = session.key.startsWith("cron:");
	return {
		label: session.label || session.key,
		items: [
			{
				id: "rename",
				label: "Rename",
				icon: "icon-code",
				disabled: !canRenameSession(session.key),
				onSelect: () => {
					renamingSessionKey.value = session.key;
				},
			},
			{
				id: "pin",
				label: pinned ? "Unpin" : "Pin",
				icon: "icon-list-plus",
				onSelect: () => togglePinned(session.key),
			},
			{
				id: "move",
				label: "Move to project\u2026",
				icon: "icon-folder",
				disabled: isCron,
				onSelect: () => {
					const row = document.querySelector<HTMLElement>(
						`.session-item[data-session-key="${CSS.escape(session.key)}"]`,
					);
					const rect = row?.getBoundingClientRect();
					projectMenu(session, { x: rect ? rect.right : 80, y: rect ? rect.top : 80 });
				},
			},
			{
				id: "fork",
				label: "Fork",
				icon: "icon-git-fork",
				disabled: isCron,
				separatorBefore: true,
				onSelect: () => void forkSession(session.key),
			},
			{
				id: "share",
				label: "Share\u2026",
				icon: "icon-share",
				disabled: isCron,
				onSelect: () => void shareSession(session.key),
			},
			{
				id: "archive",
				label: session.archived ? "Unarchive" : "Archive",
				icon: "icon-layers",
				disabled: session.key === "main",
				onSelect: () => void setArchived(session.key, !session.archived),
			},
			{
				id: "delete",
				label: "Delete",
				icon: "icon-x-circle",
				danger: true,
				separatorBefore: true,
				disabled: session.key === "main",
				onSelect: () => deleteSessionWithUndo(session.key),
			},
		],
	};
}
