// Loaded synchronously in <head> so the saved theme applies before first paint.
(() => {
	let choice = "auto";
	try {
		choice = localStorage.getItem("antre-site-theme") || "auto";
	} catch {}
	const dark =
		choice === "dark" ||
		(choice === "auto" && window.matchMedia("(prefers-color-scheme: dark)").matches);
	document.documentElement.dataset.theme = dark ? "dark" : "light";
})();
