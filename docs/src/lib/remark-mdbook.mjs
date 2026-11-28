function visit(node, visitor) {
  visitor(node)

  if (!node || !Array.isArray(node.children)) {
    return
  }

  for (const child of node.children) {
    visit(child, visitor)
  }
}

function escapeHtml(value) {
  return value
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
}

function renderInlineMarkdown(value, base) {
  const tokens = []
  let text = escapeHtml(value)

  text = text.replace(/`([^`]+)`/g, (_, code) => {
    const token = `@@TOKEN${tokens.length}@@`
    tokens.push(`<code>${code}</code>`)
    return token
  })
  text = text.replace(/\[([^\]]+)\]\(([^)]+)\)/g, (_, label, href) => {
    const token = `@@TOKEN${tokens.length}@@`
    tokens.push(`<a href="${escapeHtml(rewriteMarkdownHref(href, base))}">${label}</a>`)
    return token
  })
  text = text.replace(/\*\*([^*]+)\*\*/g, "<strong>$1</strong>")

  return tokens.reduce((rendered, token, index) => rendered.replace(`@@TOKEN${index}@@`, token), text)
}

function renderParagraph(lines, base) {
  return `<p>${lines.map((line) => renderInlineMarkdown(line, base)).join("<br>")}</p>`
}

function renderList(lines, base) {
  const items = lines
    .map((line) => line.replace(/^\s*[-*]\s+/, "").trim())
    .filter(Boolean)
    .map((line) => `<li>${renderInlineMarkdown(line, base)}</li>`)
    .join("")

  return `<ul>${items}</ul>`
}

function renderAdmonitionBody(value, base) {
  return value
    .split(/\n{2,}/)
    .map((block) => block.trim())
    .filter(Boolean)
    .map((block) => {
      const lines = block.split("\n")
      return lines.every((line) => /^\s*[-*]\s+/.test(line)) ? renderList(lines, base) : renderParagraph(lines, base)
    })
    .join("\n")
}

function parseAdmonition(meta) {
  const value = meta || "note"
  const kind = value.split(/\s+/)[0] || "note"
  const titleMatch = value.match(/title=["']([^"']+)["']/)
  const fallbackTitle = kind.replace(/^./, (char) => char.toUpperCase())

  return {
    kind,
    title: titleMatch ? titleMatch[1] : fallbackTitle,
  }
}

function renderAdmonition(node, base) {
  const { kind, title } = parseAdmonition(node.meta)
  const body = renderAdmonitionBody(node.value, base)

  return `<aside class="admonition admonition-${escapeHtml(kind)}"><div class="admonition-title">${escapeHtml(title)}</div><div class="admonition-body">${body}</div></aside>`
}

export function rewriteMarkdownHref(href, base = "") {
  if (!href || href.startsWith("http://") || href.startsWith("https://") || href.startsWith("#")) {
    return href
  }

  const [rawPath, fragment] = href.split("#")
  const path = rawPath.replace(/^\.\//, "").replace(/^(\.\.\/)+/, "")
  if (!path.endsWith(".md")) {
    return href
  }

  const rewritten = path === "index.md" ? `${base}/index.html` : `${base}/${path.replace(/\.md$/, ".html")}`
  return fragment ? `${rewritten}#${fragment}` : rewritten
}

export function mdbookCompat({ base = "" } = {}) {
  return (tree) => {
    visit(tree, (node) => {
      if (node.type === "link") {
        node.url = rewriteMarkdownHref(node.url, base)
      }

      if (node.type === "code" && node.lang === "admonish") {
        node.type = "html"
        node.value = renderAdmonition(node, base)
        delete node.lang
        delete node.meta
      }
    })
  }
}
