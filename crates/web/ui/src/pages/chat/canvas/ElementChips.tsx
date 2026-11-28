// ── Element context chips in the composer ───────────────────

import { useTranslation } from "../../../i18n";
import { elementLabel } from "./prompts";
import { elementContexts, removeElementContext } from "./store";

export function ElementChips() {
	const { t } = useTranslation("canvas");
	const contexts = elementContexts.value;
	if (contexts.length === 0) return null;
	return (
		<ul className="m-0 flex list-none flex-wrap gap-1 px-2 pt-2" aria-label={t("elementContexts")}>
			{contexts.map((context) => {
				const label = context.element.mid ?? context.element.tag;
				return (
					<li
						key={context.id}
						className="an-chip an-chip--file max-w-full"
						title={`${context.designTitle} · ${elementLabel(context.element)}\n${context.element.text}`}
					>
						<span className="an-i an-i-frame an-i--sm" aria-hidden="true" />
						<span className="truncate">{label}</span>
						<button
							type="button"
							className="ml-1 inline-flex cursor-pointer border-0 bg-transparent p-0 text-ink-muted"
							aria-label={t("removeElement", { element: label })}
							onClick={() => removeElementContext(context.id)}
						>
							<span className="an-i an-i-x an-i--sm" aria-hidden="true" />
						</button>
					</li>
				);
			})}
		</ul>
	);
}
