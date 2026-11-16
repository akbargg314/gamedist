// Tiny, deliberately limited Markdown renderer for game descriptions and
// changelogs, which come from untrusted servers/developers.
//
// Safety model: every piece of source text is HTML-escaped before any markup
// is added, and the only tags we ever emit are a fixed allow-list. Links are
// emitted only for http(s)/mailto targets and carry `data-external` so the
// UI routes clicks to the system browser instead of navigating the webview.
// Images are not rendered (they would allow tracking pixels); their alt text
// is shown instead.
//
// Supported: ATX headings, paragraphs, hard line breaks are soft, `-`/`*`/`+`
// and `1.` lists (one level), > blockquotes, ``` fenced code, `inline code`,
// **bold**, *italic*/_italic_, ~~strike~~, [text](url), <autolinks>, bare
// https:// URLs, and horizontal rules.

export function escapeHtml(s: string): string {
  return s
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
    .replace(/'/g, '&#39;');
}

/** Returns the URL if it is safe to link to, else null. Input is raw (unescaped). */
export function safeHref(raw: string): string | null {
  const url = raw.trim();
  if (!/^(https?:\/\/|mailto:)/i.test(url)) return null;
  // Reject whitespace/control characters that could confuse parsers.
  if (/[\s\u0000-\u001f\u007f]/.test(url)) return null;
  try {
    const u = new URL(url);
    if (!['http:', 'https:', 'mailto:'].includes(u.protocol)) return null;
  } catch {
    return null;
  }
  return url;
}

function link(href: string, textHtml: string): string {
  return `<a href="${escapeHtml(href)}" data-external rel="noopener noreferrer" target="_blank">${textHtml}</a>`;
}

/** Inline formatting. Input is raw text; output is safe HTML. */
export function renderInline(src: string): string {
  // Pull code spans and links out first so their contents are not formatted.
  const slots: string[] = [];
  const hold = (html: string) => `\u0000${slots.push(html) - 1}\u0000`;
  // Strip any NULs from input so placeholders cannot be forged.
  let s = src.replace(/\u0000/g, '');

  s = s.replace(/`([^`\n]+)`/g, (_, code: string) => hold(`<code>${escapeHtml(code)}</code>`));
  // Images: show alt text only.
  s = s.replace(/!\[([^\]]*)\]\(([^)\s]*)(?:\s+"[^"]*")?\)/g, (_, alt: string) => hold(escapeHtml(alt)));
  s = s.replace(/\[([^\]]+)\]\(([^)\s]+)(?:\s+"[^"]*")?\)/g, (whole: string, text: string, href: string) => {
    const ok = safeHref(href);
    const inner = renderEmphasis(escapeHtml(text));
    return hold(ok ? link(ok, inner) : inner);
  });
  s = s.replace(/<((?:https?:\/\/|mailto:)[^>\s]+)>/gi, (whole: string, href: string) => {
    const ok = safeHref(href);
    return hold(ok ? link(ok, escapeHtml(href)) : escapeHtml(whole));
  });
  s = s.replace(/\bhttps?:\/\/[^\s<>()]+[^\s<>().,;:!?'"]/g, (href: string) => {
    const ok = safeHref(href);
    return hold(ok ? link(ok, escapeHtml(href)) : escapeHtml(href));
  });

  let html = renderEmphasis(escapeHtml(s));
  html = html.replace(/\u0000(\d+)\u0000/g, (_, i: string) => slots[Number(i)]);
  return html;
}

/** Emphasis on already-escaped text. */
function renderEmphasis(html: string): string {
  return html
    .replace(/\*\*([^*\n]+)\*\*/g, '<strong>$1</strong>')
    .replace(/__([^_\n]+)__/g, '<strong>$1</strong>')
    .replace(/~~([^~\n]+)~~/g, '<del>$1</del>')
    .replace(/(^|[^*\w])\*([^*\n]+)\*(?!\w)/g, '$1<em>$2</em>')
    .replace(/(^|[^_\w])_([^_\n]+)_(?!\w)/g, '$1<em>$2</em>');
}

export function renderMarkdown(src: string): string {
  const lines = (src ?? '').replace(/\r\n?/g, '\n').split('\n');
  const out: string[] = [];
  let para: string[] = [];
  let list: { ordered: boolean; items: string[] } | null = null;
  let quote: string[] | null = null;

  const flushPara = () => {
    if (para.length) out.push(`<p>${renderInline(para.join(' '))}</p>`);
    para = [];
  };
  const flushList = () => {
    if (list) {
      const tag = list.ordered ? 'ol' : 'ul';
      out.push(`<${tag}>${list.items.map((i) => `<li>${renderInline(i)}</li>`).join('')}</${tag}>`);
    }
    list = null;
  };
  const flushQuote = () => {
    if (quote) out.push(`<blockquote>${renderMarkdown(quote.join('\n'))}</blockquote>`);
    quote = null;
  };
  const flushAll = () => {
    flushPara();
    flushList();
    flushQuote();
  };

  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];

    const fence = /^\s*(```|~~~)/.exec(line);
    if (fence) {
      flushAll();
      const code: string[] = [];
      i++;
      while (i < lines.length && !lines[i].trimStart().startsWith(fence[1])) code.push(lines[i++]);
      out.push(`<pre><code>${escapeHtml(code.join('\n'))}</code></pre>`);
      continue;
    }

    if (/^\s*>/.test(line)) {
      flushPara();
      flushList();
      (quote ??= []).push(line.replace(/^\s*>\s?/, ''));
      continue;
    } else if (quote) {
      flushQuote();
    }

    if (!line.trim()) {
      flushPara();
      flushList();
      continue;
    }

    const h = /^\s{0,3}(#{1,6})\s+(.*?)\s*#*\s*$/.exec(line);
    if (h) {
      flushAll();
      // Page already has an h1/h2; shift document headings down.
      const level = Math.min(6, h[1].length + 2);
      out.push(`<h${level}>${renderInline(h[2])}</h${level}>`);
      continue;
    }

    if (/^\s{0,3}([-*_])(\s*\1){2,}\s*$/.test(line)) {
      flushAll();
      out.push('<hr>');
      continue;
    }

    const ul = /^\s*[-*+]\s+(.*)$/.exec(line);
    const ol = /^\s*\d{1,9}[.)]\s+(.*)$/.exec(line);
    if (ul || ol) {
      flushPara();
      const ordered = !!ol;
      if (list && list.ordered !== ordered) flushList();
      (list ??= { ordered, items: [] }).items.push((ul ?? ol)![1]);
      continue;
    }

    if (list && /^\s{2,}\S/.test(line)) {
      // Continuation of the previous list item.
      const items: string[] = (list as { items: string[] }).items;
      items[items.length - 1] += ' ' + line.trim();
      continue;
    }

    flushList();
    para.push(line.trim());
  }
  flushAll();
  return out.join('\n');
}
