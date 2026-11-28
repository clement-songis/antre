// ── Mods section (placeholder until the mods runtime lands) ───

import type { VNode } from "preact";
import { SectionHeading } from "../../components/forms";

interface ModKind {
	id: string;
	title: string;
	description: string;
}

const MOD_KINDS: ModKind[] = [
	{ id: "theme", title: "Themes", description: "Colors and type for the whole app." },
	{ id: "pane", title: "Panes", description: "A view that docks beside the chat and updates live." },
	{ id: "renderer", title: "Tool renderers", description: "Show a tool's input and output your way." },
	{ id: "command", title: "Slash commands", description: "New / commands for the composer." },
	{ id: "hook", title: "Hooks", description: "Run code before or after a tool call." },
];

export function ModsSection(): VNode {
	return (
		<div className="flex-1 flex flex-col min-w-0 p-4 gap-4 overflow-y-auto">
			<SectionHeading
				title="Mods"
				subtitle="Change how the app looks and behaves. Each mod declares what it can touch and runs in an isolated frame."
			/>
			<div className="mods-placeholder an-card an-card__pad flex flex-col gap-3 max-w-[720px]">
				<div className="flex items-center gap-2">
					<span className="icon icon-lg icon-puzzle text-ink-muted" aria-hidden="true" />
					<span className="font-display text-heading text-ink-strong">No mods installed</span>
					<span className="an-badge an-badge--info">Coming soon</span>
				</div>
				<p className="text-small text-ink-muted m-0">
					Installing, signing and reviewing mods is not available in this build yet. When it is, installed mods and the
					permissions each one asks for will be listed here.
				</p>
			</div>
			<ul className="mods-kinds grid gap-3 max-w-[720px] list-none p-0 m-0">
				{MOD_KINDS.map((kind) => (
					<li key={kind.id} className="an-choice is-disabled" data-mod-kind={kind.id}>
						<span className="an-choice__title">{kind.title}</span>
						<span className="an-choice__meta">{kind.description}</span>
					</li>
				))}
			</ul>
		</div>
	);
}
