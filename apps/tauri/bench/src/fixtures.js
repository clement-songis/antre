// Deterministic content: a long assistant reply, a transcript, a heavy
// HTML design. Same seed, same bytes, on every engine.

import { rng } from "./metrics.js";

const WORDS = (
	"the agent reads files plans changes runs tests and reports back with a summary of what " +
	"changed why it changed and which commands were used to verify the result before the user " +
	"reviews the diff gateway session provider model stream token sandbox container memory " +
	"tool approval channel webhook cron schedule project workspace vault passkey canvas design"
).split(" ");

function sentence(random, min = 8, max = 22) {
	const length = min + Math.floor(random() * (max - min));
	const words = Array.from({ length }, () => WORDS[Math.floor(random() * WORDS.length)]);
	if (random() < 0.3) words[Math.floor(random() * length)] = `\`${words[0]}_${words[1]}\``;
	if (random() < 0.2) words[Math.floor(random() * length)] = `**${words[2]}**`;
	const text = words.join(" ");
	return `${text[0].toUpperCase()}${text.slice(1)}.`;
}

function paragraph(random, sentences = 4) {
	return Array.from({ length: 2 + Math.floor(random() * sentences) }, () => sentence(random)).join(" ");
}

const RUST = `use std::collections::HashMap;

/// Group sessions by project and keep the newest first.
pub fn group_sessions(sessions: &[Session]) -> HashMap<String, Vec<&Session>> {
    let mut groups: HashMap<String, Vec<&Session>> = HashMap::new();
    for session in sessions {
        groups.entry(session.project.clone()).or_default().push(session);
    }
    for list in groups.values_mut() {
        list.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    }
    groups
}

#[derive(Debug, Clone)]
pub struct Session {
    pub id: String,
    pub project: String,
    pub updated_at: i64,
}`;

const TS = `export async function streamReply(sessionId: string, onDelta: (text: string) => void): Promise<string> {
	const response = await fetch(\`/api/sessions/\${sessionId}/stream\`, { method: "POST" });
	if (!response.body) throw new Error("no body");
	const reader = response.body.getReader();
	const decoder = new TextDecoder();
	let full = "";
	for (;;) {
		const { value, done } = await reader.read();
		if (done) break;
		const chunk = decoder.decode(value, { stream: true });
		full += chunk;
		onDelta(chunk);
	}
	return full;
}`;

const PY = `def summarize(rows: list[dict]) -> dict:
    totals = {}
    for row in rows:
        key = row["provider"]
        totals.setdefault(key, {"tokens": 0, "cost": 0.0})
        totals[key]["tokens"] += row["tokens"]
        totals[key]["cost"] += row["cost"]
    return dict(sorted(totals.items(), key=lambda item: -item[1]["cost"]))`;

const BASH = `cargo build --release -p moltis
./target/release/moltis --bind 127.0.0.1 --port 13131 --no-tls gateway &
curl -fsS http://127.0.0.1:13131/health | jq .`;

const CODE = [
	["rust", RUST],
	["typescript", TS],
	["python", PY],
	["bash", BASH],
];

function table(random) {
	const rows = Array.from({ length: 4 + Math.floor(random() * 4) }, (_, i) => {
		const cells = [`row ${i + 1}`, `${Math.floor(random() * 900)} ms`, `${Math.floor(random() * 100)}%`, WORDS[i]];
		return `| ${cells.join(" | ")} |`;
	});
	return ["| Step | Time | Share | Note |", "|---|---:|---:|---|", ...rows].join("\n");
}

function list(random) {
	return Array.from({ length: 3 + Math.floor(random() * 4) }, () => `- ${sentence(random, 5, 12)}`).join("\n");
}

/** A long reply (~`targetChars` characters) with headings, lists, tables and code. */
export function longReply(targetChars = 16000, seed = 7) {
	const random = rng(seed);
	const blocks = ["## Summary", paragraph(random, 5)];
	let section = 1;
	while (blocks.join("\n\n").length < targetChars) {
		blocks.push(`### ${section}. ${sentence(random, 3, 6).slice(0, -1)}`);
		blocks.push(paragraph(random));
		const pick = random();
		if (pick < 0.45) {
			const [lang, code] = CODE[section % CODE.length];
			blocks.push(`\`\`\`${lang}\n${code}\n\`\`\``);
		} else if (pick < 0.7) {
			blocks.push(list(random));
		} else if (pick < 0.85) {
			blocks.push(table(random));
		} else {
			blocks.push(`> ${sentence(random)}`);
		}
		blocks.push(paragraph(random, 3));
		section++;
	}
	return blocks.join("\n\n");
}

/** Split text into "tokens" of ~4 characters (the usual rule of thumb). */
export function tokenize(text, charsPerToken = 4) {
	const tokens = [];
	for (let i = 0; i < text.length; i += charsPerToken) tokens.push(text.slice(i, i + charsPerToken));
	return tokens;
}

/** A transcript of `count` messages, alternating user and assistant. */
export function transcript(count = 2000, seed = 11) {
	const random = rng(seed);
	return Array.from({ length: count }, (_, i) => {
		if (i % 2 === 0) return { role: "user", text: sentence(random, 6, 30) };
		const parts = [paragraph(random, 3)];
		const pick = random();
		if (pick < 0.2) {
			const [lang, code] = CODE[Math.floor(random() * CODE.length)];
			parts.push(`\`\`\`${lang}\n${code}\n\`\`\``);
		} else if (pick < 0.45) {
			parts.push(list(random));
		} else if (pick < 0.5) {
			parts.push(table(random));
		}
		if (random() < 0.5) parts.push(paragraph(random, 2));
		return { role: "assistant", text: parts.join("\n\n") };
	});
}

const PALETTE = ["#6366f1", "#ec4899", "#14b8a6", "#f59e0b", "#8b5cf6", "#06b6d4", "#ef4444", "#22c55e"];

function sparkline(random, points = 240, width = 320, height = 80) {
	let y = height / 2;
	const coords = Array.from({ length: points }, (_, i) => {
		y = Math.max(4, Math.min(height - 4, y + (random() - 0.5) * 12));
		return `${((i / (points - 1)) * width).toFixed(1)},${y.toFixed(1)}`;
	});
	return `<svg viewBox="0 0 ${width} ${height}" class="spark"><defs><linearGradient id="g${points}${Math.floor(random() * 1e6)}" x1="0" x2="0" y1="0" y2="1"><stop offset="0" stop-color="currentColor" stop-opacity=".45"/><stop offset="1" stop-color="currentColor" stop-opacity="0"/></linearGradient></defs><polyline fill="none" stroke="currentColor" stroke-width="2" points="${coords.join(" ")}"/></svg>`;
}

/**
 * A heavy, self-contained HTML design like an agent would generate on a
 * canvas: a marketing page plus a dashboard, with gradients, shadows,
 * backdrop blur, CSS animations, SVG charts and a <canvas> chart.
 */
export function heavyDesign(seed = 3, cards = 240) {
	const random = rng(seed);
	const cardHtml = Array.from({ length: cards }, (_, i) => {
		const color = PALETTE[i % PALETTE.length];
		return `<article class="card" style="--accent:${color}">
  <header><span class="dot"></span><h3>${sentence(random, 2, 4).slice(0, -1)}</h3><span class="badge">${Math.floor(random() * 99)}%</span></header>
  <p>${sentence(random, 10, 20)}</p>
  <div class="chart" style="color:${color}">${sparkline(random)}</div>
  <footer><button>Open</button><button class="ghost">Share</button><span class="avatar-stack">${'<i></i>'.repeat(4)}</span></footer>
</article>`;
	}).join("\n");
	const features = Array.from(
		{ length: 12 },
		(_, i) =>
			`<div class="feature"><div class="icon" style="background:${PALETTE[i % 8]}"></div><h4>${sentence(random, 2, 3).slice(0, -1)}</h4><p>${sentence(random, 12, 18)}</p></div>`,
	).join("");
	return `<!doctype html>
<html><head><meta charset="utf-8"><title>Design</title>
<style>
*{box-sizing:border-box}body{margin:0;font:15px/1.5 -apple-system,system-ui,sans-serif;color:#0f172a;background:#f8fafc}
.hero{position:relative;overflow:hidden;padding:120px 64px;background:radial-gradient(1200px 600px at 10% -10%,#c7d2fe,transparent),radial-gradient(900px 500px at 90% 10%,#fbcfe8,transparent),linear-gradient(180deg,#eef2ff,#f8fafc)}
.hero h1{font-size:64px;line-height:1.05;margin:0 0 24px;letter-spacing:-.03em;background:linear-gradient(90deg,#4f46e5,#db2777,#0d9488);-webkit-background-clip:text;background-clip:text;color:transparent}
.blob{position:absolute;width:420px;height:420px;border-radius:50%;filter:blur(60px);opacity:.55;animation:float 9s ease-in-out infinite}
.blob.a{background:#818cf8;top:-120px;right:10%}.blob.b{background:#f472b6;bottom:-160px;left:20%;animation-delay:-3s}
@keyframes float{50%{transform:translate(40px,30px) scale(1.08)}}
.glass{position:relative;display:inline-flex;gap:16px;padding:18px 24px;border-radius:20px;background:rgba(255,255,255,.55);backdrop-filter:blur(18px) saturate(160%);-webkit-backdrop-filter:blur(18px) saturate(160%);box-shadow:0 20px 60px -20px rgba(15,23,42,.35),inset 0 1px 0 rgba(255,255,255,.8)}
.features{display:grid;grid-template-columns:repeat(4,1fr);gap:24px;padding:64px}
.feature{padding:24px;border-radius:18px;background:#fff;box-shadow:0 1px 2px rgba(0,0,0,.05),0 12px 32px -12px rgba(15,23,42,.18);transition:transform .2s}
.feature:hover{transform:translateY(-4px)}.icon{width:40px;height:40px;border-radius:12px;margin-bottom:12px;box-shadow:0 8px 20px -6px currentColor}
.grid{display:grid;grid-template-columns:repeat(auto-fill,minmax(300px,1fr));gap:20px;padding:0 64px 64px}
.card{border-radius:16px;padding:18px;background:linear-gradient(180deg,#fff,#fafbff);border:1px solid rgba(15,23,42,.06);box-shadow:0 1px 1px rgba(0,0,0,.04),0 10px 30px -14px color-mix(in srgb,var(--accent) 60%,transparent)}
.card header{display:flex;align-items:center;gap:10px}.card h3{flex:1;margin:0;font-size:15px}
.dot{width:10px;height:10px;border-radius:50%;background:var(--accent);box-shadow:0 0 0 4px color-mix(in srgb,var(--accent) 20%,transparent);animation:pulse 2s infinite}
@keyframes pulse{50%{box-shadow:0 0 0 8px color-mix(in srgb,var(--accent) 5%,transparent)}}
.badge{font-size:12px;padding:2px 8px;border-radius:999px;background:color-mix(in srgb,var(--accent) 14%,white);color:var(--accent);font-weight:600}
.spark{width:100%;height:80px;display:block}.card p{color:#475569;font-size:13px}
.card footer{display:flex;gap:8px;align-items:center}button{border:0;border-radius:10px;padding:6px 12px;background:var(--accent,#4f46e5);color:#fff;font-weight:600}
button.ghost{background:transparent;color:#334155;box-shadow:inset 0 0 0 1px #cbd5e1}
.avatar-stack{margin-left:auto;display:flex}.avatar-stack i{width:22px;height:22px;border-radius:50%;margin-left:-6px;border:2px solid #fff;background:conic-gradient(var(--accent),#fde68a,var(--accent))}
.big{margin:0 64px 64px;padding:24px;border-radius:20px;background:#0f172a;color:#e2e8f0}
canvas{width:100%;height:320px;display:block}
.marquee{white-space:nowrap;overflow:hidden;padding:16px 0;background:#111827;color:#a5b4fc}.marquee span{display:inline-block;animation:scroll 20s linear infinite}
@keyframes scroll{to{transform:translateX(-50%)}}
</style></head>
<body>
<section class="hero"><div class="blob a"></div><div class="blob b"></div>
<h1>${sentence(random, 4, 7)}</h1><p style="max-width:640px;font-size:20px;color:#334155">${paragraph(random, 2)}</p>
<div class="glass"><button>Get started</button><button class="ghost">Watch demo</button></div></section>
<div class="marquee"><span>${Array.from({ length: 40 }, () => WORDS[Math.floor(random() * WORDS.length)]).join(" • ").repeat(2)}</span></div>
<section class="features">${features}</section>
<section class="big"><h2>Throughput</h2><canvas id="c" width="1600" height="640"></canvas></section>
<section class="grid">${cardHtml}</section>
<script>
const frames=[];let last=0;
function tick(t){frames.push(t);requestAnimationFrame(tick)}requestAnimationFrame(tick);
const c=document.getElementById("c"),x=c.getContext("2d");let phase=0;
function draw(){x.clearRect(0,0,c.width,c.height);for(let s=0;s<6;s++){x.beginPath();x.strokeStyle=["#818cf8","#f472b6","#2dd4bf","#fbbf24","#a78bfa","#22d3ee"][s];x.lineWidth=3;for(let i=0;i<=400;i++){const px=i*4,py=320+Math.sin(i/18+phase+s)*90*Math.cos(i/70+s)+s*12;i?x.lineTo(px,py):x.moveTo(px,py)}x.stroke()}phase+=.04;requestAnimationFrame(draw)}draw();
function stats(from,to,refresh){const ts=frames.filter(t=>t>=from&&t<=to);const iv=ts.slice(1).map((t,i)=>t-ts[i]);const dur=ts.length>1?ts[ts.length-1]-ts[0]:0;const dropped=iv.reduce((s,d)=>s+Math.max(0,Math.round(d/refresh)-1),0);const sorted=[...iv].sort((a,b)=>a-b);return{durationMs:Math.round(dur),frames:ts.length,fps:dur?Math.round((ts.length-1)*10000/dur)/10:0,droppedFrames:dropped,droppedPct:dur?Math.round(1000*dropped/(dur/refresh))/10:0,framesOver50ms:iv.filter(d=>d>50).length,worstFrameMs:Math.round(sorted[sorted.length-1]||0),frameP95Ms:Math.round((sorted[Math.ceil(.95*sorted.length)-1]||0)*10)/10}}
addEventListener("message",async e=>{const m=e.data;if(!m||m.type!=="bench")return;
 if(m.cmd==="idle"){const a=performance.now();await new Promise(r=>setTimeout(r,m.ms));parent.postMessage({type:"bench",cmd:"idle",stats:stats(a,performance.now(),m.refresh)},"*")}
 if(m.cmd==="scroll"){const a=performance.now();const max=document.documentElement.scrollHeight-innerHeight;await new Promise(r=>{const start=performance.now();function step(t){const p=Math.min(1,(t-start)/m.ms);scrollTo(0,max*p);p<1?requestAnimationFrame(step):r()}requestAnimationFrame(step)});parent.postMessage({type:"bench",cmd:"scroll",stats:stats(a,performance.now(),m.refresh),scrollHeight:document.documentElement.scrollHeight,elements:document.getElementsByTagName("*").length},"*")}
});
parent.postMessage({type:"bench",cmd:"ready",t:performance.now()},"*");
</script>
</body></html>`;
}
