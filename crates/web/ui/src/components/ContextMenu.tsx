// ── Context menu primitive ──────────────────────────────────
//
// One host renders whichever menu is open: a popover at the pointer on
// desktop, a bottom sheet on phones or after a touch long-press. Menus open
// from a right-click, a long-press, or a "…" button, and are keyboard
// operable (arrows, Home/End, Enter, Esc). Styling is token-based only.

import { signal } from "@preact/signals";
import type { ComponentChildren, JSX } from "preact";
import { Fragment, render } from "preact";
import { useEffect, useLayoutEffect, useRef, useState } from "preact/hooks";
import { t } from "../i18n";

export interface MenuItem {
	id: string;
	label: string;
	/** Icon class from the mask icon set. */
	icon?: string;
	danger?: boolean;
	disabled?: boolean;
	/** Tooltip explaining the action. */
	title?: string;
	/** Draw a divider above this item. */
	separatorBefore?: boolean;
	onSelect: () => void;
}

export interface MenuSpec {
	label: string;
	items: MenuItem[];
}

interface OpenMenu extends MenuSpec {
	x: number;
	y: number;
	sheet: boolean;
	returnFocus: HTMLElement | null;
}

export const openMenuState = signal<OpenMenu | null>(null);

const LONG_PRESS_MS = 500;
const LONG_PRESS_SLOP_PX = 10;
const PHONE_QUERY = "(max-width: 767px)";

function prefersSheet(fromTouch: boolean): boolean {
	return fromTouch || (typeof window.matchMedia === "function" && window.matchMedia(PHONE_QUERY).matches);
}

export function openContextMenu(
	spec: MenuSpec,
	at: { x: number; y: number } | HTMLElement,
	options: { fromTouch?: boolean; returnFocus?: HTMLElement | null } = {},
): void {
	const visible = spec.items.filter(Boolean);
	if (visible.length === 0) return;
	let x: number;
	let y: number;
	if (at instanceof HTMLElement) {
		const rect = at.getBoundingClientRect();
		x = rect.right;
		y = rect.bottom;
	} else {
		x = at.x;
		y = at.y;
	}
	openMenuState.value = {
		...spec,
		items: visible,
		x,
		y,
		sheet: prefersSheet(options.fromTouch === true),
		returnFocus: options.returnFocus ?? (document.activeElement as HTMLElement | null),
	};
}

export function closeContextMenu(): void {
	const open = openMenuState.value;
	openMenuState.value = null;
	open?.returnFocus?.focus?.();
}

/**
 * Props that open `getMenu()` on right-click and on a touch long-press.
 * Spread them on the element the menu belongs to.
 */
export function contextMenuTrigger(getMenu: () => MenuSpec | null): {
	onContextMenu: (e: MouseEvent) => void;
	onTouchStart: (e: TouchEvent) => void;
	onTouchMove: (e: TouchEvent) => void;
	onTouchEnd: (e: TouchEvent) => void;
	onTouchCancel: () => void;
	onClickCapture: (e: MouseEvent) => void;
} {
	let timer: ReturnType<typeof setTimeout> | null = null;
	let origin: { x: number; y: number } | null = null;
	let fired = false;
	const cancel = () => {
		if (timer) clearTimeout(timer);
		timer = null;
		origin = null;
	};
	return {
		onContextMenu: (e) => {
			const menu = getMenu();
			if (!menu) return;
			e.preventDefault();
			e.stopPropagation();
			openContextMenu(menu, { x: e.clientX, y: e.clientY }, { returnFocus: e.currentTarget as HTMLElement });
		},
		onTouchStart: (e) => {
			const touch = e.touches[0];
			if (!touch || e.touches.length > 1) return;
			fired = false;
			origin = { x: touch.clientX, y: touch.clientY };
			const target = e.currentTarget as HTMLElement;
			timer = setTimeout(() => {
				const menu = getMenu();
				timer = null;
				if (!(menu && origin)) return;
				fired = true;
				openContextMenu(menu, origin, { fromTouch: true, returnFocus: target });
			}, LONG_PRESS_MS);
		},
		onTouchMove: (e) => {
			const touch = e.touches[0];
			if (!(origin && touch)) return;
			if (Math.hypot(touch.clientX - origin.x, touch.clientY - origin.y) > LONG_PRESS_SLOP_PX) cancel();
		},
		onTouchEnd: (e) => {
			cancel();
			// A long-press must not also activate the row (open the session).
			if (fired) e.preventDefault();
		},
		onTouchCancel: cancel,
		onClickCapture: (e) => {
			if (!fired) return;
			fired = false;
			e.preventDefault();
			e.stopPropagation();
		},
	};
}

interface MenuButtonProps {
	getMenu: () => MenuSpec | null;
	label: string;
	className?: string;
	children?: ComponentChildren;
}

/** The "…" button that opens the same menu as a right-click. */
export function MenuButton({ getMenu, label, className = "", children }: MenuButtonProps) {
	const open = openMenuState.value;
	const ref = useRef<HTMLButtonElement>(null);
	const expanded = !!open && open.returnFocus === ref.current;
	return (
		<button
			ref={ref}
			type="button"
			className={`menu-trigger-btn ${className}`.trim()}
			aria-haspopup="menu"
			aria-expanded={expanded}
			aria-label={label}
			title={label}
			onClick={(e) => {
				e.preventDefault();
				e.stopPropagation();
				const menu = getMenu();
				if (menu && ref.current) openContextMenu(menu, ref.current, { returnFocus: ref.current });
			}}
		>
			{children ?? <span className="icon icon-sm icon-menu-dots" aria-hidden="true" />}
		</button>
	);
}

function focusItem(menu: HTMLElement | null, index: number): void {
	const items = menu ? [...menu.querySelectorAll<HTMLButtonElement>('[role="menuitem"]:not([disabled])')] : [];
	if (items.length === 0) return;
	items[(index + items.length) % items.length].focus();
}

function currentIndex(menu: HTMLElement | null): number {
	const items = menu ? [...menu.querySelectorAll<HTMLButtonElement>('[role="menuitem"]:not([disabled])')] : [];
	return items.indexOf(document.activeElement as HTMLButtonElement);
}

export function ContextMenuHost() {
	const open = openMenuState.value;
	const menuRef = useRef<HTMLDivElement>(null);
	const [position, setPosition] = useState<{ left: number; top: number } | null>(null);

	useLayoutEffect(() => {
		setPosition(null);
		if (!open || open.sheet || !menuRef.current) return;
		const rect = menuRef.current.getBoundingClientRect();
		const margin = 8;
		const left = Math.max(margin, Math.min(open.x, window.innerWidth - rect.width - margin));
		const top = open.y + rect.height + margin > window.innerHeight ? Math.max(margin, open.y - rect.height) : open.y;
		setPosition({ left, top });
	}, [open]);

	// Focus once the menu is placed: a popover stays hidden until measured.
	useEffect(() => {
		if (open && (open.sheet || position)) focusItem(menuRef.current, 0);
	}, [open, position]);

	useEffect(() => {
		if (!open) return;
		const close = () => closeContextMenu();
		// Focus reaches the menu only after it is positioned; Esc must work before that.
		const onEscape = (e: KeyboardEvent) => {
			if (e.key !== "Escape") return;
			e.preventDefault();
			e.stopPropagation();
			closeContextMenu();
		};
		window.addEventListener("resize", close);
		window.addEventListener("blur", close);
		window.addEventListener("keydown", onEscape, true);
		return () => {
			window.removeEventListener("resize", close);
			window.removeEventListener("blur", close);
			window.removeEventListener("keydown", onEscape, true);
		};
	}, [open]);

	if (!open) return null;

	const onKeyDown = (e: JSX.TargetedKeyboardEvent<HTMLDivElement>) => {
		const index = currentIndex(menuRef.current);
		if (e.key === "ArrowDown") focusItem(menuRef.current, index + 1);
		else if (e.key === "ArrowUp") focusItem(menuRef.current, index - 1);
		else if (e.key === "Home") focusItem(menuRef.current, 0);
		else if (e.key === "End") focusItem(menuRef.current, -1);
		else if (e.key === "Escape" || e.key === "Tab") closeContextMenu();
		else return;
		e.preventDefault();
		e.stopPropagation();
	};

	const style = open.sheet
		? undefined
		: {
				left: `${position?.left ?? open.x}px`,
				top: `${position?.top ?? open.y}px`,
				visibility: position ? "visible" : "hidden",
			};

	return (
		<div
			className={`context-menu-backdrop${open.sheet ? " is-sheet" : ""}`}
			onMouseDown={(e) => {
				if (e.target === e.currentTarget) closeContextMenu();
			}}
			onContextMenu={(e) => {
				e.preventDefault();
				closeContextMenu();
			}}
		>
			<div
				ref={menuRef}
				className={open.sheet ? "context-menu context-sheet" : "context-menu"}
				role="menu"
				aria-label={open.label}
				style={style}
				onKeyDown={onKeyDown}
			>
				{open.sheet ? <div className="context-sheet-title">{open.label}</div> : null}
				{open.items.map((item) => (
					<Fragment key={item.id}>
						{item.separatorBefore ? <hr className="context-menu-separator" /> : null}
						<button
							type="button"
							role="menuitem"
							className={`context-menu-item${item.danger ? " is-danger" : ""}`}
							disabled={item.disabled}
							data-menu-item={item.id}
							title={item.title}
							onClick={() => {
								closeContextMenu();
								item.onSelect();
							}}
						>
							{item.icon ? <span className={`icon icon-sm ${item.icon}`} aria-hidden="true" /> : null}
							<span>{item.label}</span>
						</button>
					</Fragment>
				))}
				{open.sheet ? (
					<button type="button" className="context-menu-item context-sheet-cancel" onClick={closeContextMenu}>
						{t("common:actions.cancel")}
					</button>
				) : null}
			</div>
		</div>
	);
}

let hostMounted = false;

/** Mount the single menu host once per page. */
export function mountContextMenuHost(): void {
	if (hostMounted) return;
	hostMounted = true;
	const host = document.createElement("div");
	host.id = "contextMenuHost";
	document.body.appendChild(host);
	render(<ContextMenuHost />, host);
}
