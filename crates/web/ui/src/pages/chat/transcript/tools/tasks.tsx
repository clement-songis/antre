// ── task_list as a checklist ────────────────────────────────
//
// `task_list` results carry one task (create/update/get/claim) or a list.
// Folding every call of the session in order gives the current state of
// each task, which the pinned checklist shows live.

import { computed } from "@preact/signals";
import { t } from "../../../../i18n";
import type { ToolItem } from "../model";
import { transcriptItems } from "../store";
import { isRecord, num, resultRecord, str } from "./shared";

export type TaskStatus = "pending" | "in_progress" | "completed";

export interface TaskEntry {
	id: string;
	subject: string;
	status: TaskStatus;
	updatedAt: number;
}

function toTask(value: unknown): TaskEntry | null {
	if (!isRecord(value)) return null;
	const id = str(value.id);
	if (!id) return null;
	const status = str(value.status);
	return {
		id,
		subject: str(value.subject) || id,
		status: status === "in_progress" || status === "completed" ? status : "pending",
		updatedAt: num(value.updated_at) ?? 0,
	};
}

export function tasksFromToolItem(item: ToolItem): TaskEntry[] {
	const res = resultRecord(item);
	if (!res) return [];
	if (Array.isArray(res.tasks)) return res.tasks.map(toTask).filter((t): t is TaskEntry => t !== null);
	const single = toTask(res.task);
	return single ? [single] : [];
}

/** Every task the session's `task_list` calls mention, latest state wins. */
export const sessionTasks = computed<TaskEntry[]>(() => {
	const byId = new Map<string, TaskEntry>();
	for (const item of transcriptItems.value) {
		if (item.kind !== "tool" || item.name !== "task_list" || item.status !== "ok") continue;
		for (const task of tasksFromToolItem(item)) {
			const previous = byId.get(task.id);
			if (!previous || task.updatedAt >= previous.updatedAt) byId.set(task.id, task);
		}
	}
	return [...byId.values()];
});

const STATUS_MARK: Record<TaskStatus, string> = {
	pending: "○",
	in_progress: "◐",
	completed: "✓",
};

export function TaskChecklist({ tasks }: { tasks: TaskEntry[] }) {
	return (
		<ul className="task-checklist" aria-label="Tasks">
			{tasks.map((task) => (
				<li key={task.id} className={`task-checklist-item task-${task.status}`} data-task-status={task.status}>
					<span className="task-checklist-mark" aria-hidden="true">
						{STATUS_MARK[task.status]}
					</span>
					<span className="task-checklist-subject">{task.subject}</span>
				</li>
			))}
		</ul>
	);
}

/** Pinned under the latest message while some task is still open. */
export function PinnedTaskChecklist() {
	const tasks = sessionTasks.value;
	if (tasks.length === 0 || tasks.every((task) => task.status === "completed")) return null;
	const done = tasks.filter((task) => task.status === "completed").length;
	return (
		<section className="msg pinned-task-checklist" aria-label={t("chat:tools.taskProgress")}>
			<div className="tool-section-label">{t("chat:tools.tasksDone", { done, total: tasks.length })}</div>
			<TaskChecklist tasks={tasks} />
		</section>
	);
}
