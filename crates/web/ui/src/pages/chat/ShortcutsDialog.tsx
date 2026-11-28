// ── Keyboard shortcuts help ─────────────────────────────────

import { render } from "preact";
import { useEffect } from "preact/hooks";
import { useTranslation } from "../../i18n";
import { SHORTCUTS, shortcutsHelpOpen } from "./shortcuts";

export function ShortcutsDialog() {
	const { t } = useTranslation("chat");
	const open = shortcutsHelpOpen.value;
	useEffect(() => {
		if (!open) return;
		const onKey = (e: KeyboardEvent) => {
			if (e.key !== "Escape") return;
			e.preventDefault();
			e.stopPropagation();
			shortcutsHelpOpen.value = false;
		};
		document.addEventListener("keydown", onKey, true);
		return () => document.removeEventListener("keydown", onKey, true);
	}, [open]);
	if (!open) return null;
	const close = () => {
		shortcutsHelpOpen.value = false;
	};
	return (
		<div
			className="provider-modal-backdrop"
			onClick={(e) => {
				if (e.target === e.currentTarget) close();
			}}
		>
			<div
				className="provider-modal shortcuts-dialog w-[min(440px,94vw)]"
				role="dialog"
				aria-modal="true"
				aria-label={t("shortcuts.title")}
			>
				<div className="provider-modal-header">
					<div className="provider-item-name">{t("shortcuts.title")}</div>
					<button type="button" className="provider-btn provider-btn-secondary provider-btn-sm" onClick={close}>
						{t("rewind.close")}
					</button>
				</div>
				<div className="provider-modal-body">
					<dl className="shortcuts-list m-0 grid grid-cols-[1fr_auto] gap-x-4 gap-y-2 text-sm">
						{SHORTCUTS.map((spec) => (
							<div key={spec.id} className="contents" data-shortcut={spec.id}>
								<dt className="text-ink">{t(`shortcuts.${spec.labelKey}`)}</dt>
								<dd className="m-0 flex gap-1 justify-end">
									{spec.keys.map((key, index) => (
										// biome-ignore lint/suspicious/noArrayIndexKey: "Esc Esc" repeats a key.
										<kbd key={index} className="shortcut-key">
											{key}
										</kbd>
									))}
								</dd>
							</div>
						))}
					</dl>
				</div>
			</div>
		</div>
	);
}

let mounted = false;

export function mountShortcutsDialog(): void {
	if (mounted) return;
	mounted = true;
	const host = document.createElement("div");
	host.id = "shortcutsDialogHost";
	document.body.appendChild(host);
	render(<ShortcutsDialog />, host);
}
