import { Marked } from "marked";
import hljs from "highlight.js/lib/core";
import bash from "highlight.js/lib/languages/bash";
import css from "highlight.js/lib/languages/css";
import diff from "highlight.js/lib/languages/diff";
import go from "highlight.js/lib/languages/go";
import javascript from "highlight.js/lib/languages/javascript";
import json from "highlight.js/lib/languages/json";
import python from "highlight.js/lib/languages/python";
import rust from "highlight.js/lib/languages/rust";
import sql from "highlight.js/lib/languages/sql";
import swift from "highlight.js/lib/languages/swift";
import typescript from "highlight.js/lib/languages/typescript";
import xml from "highlight.js/lib/languages/xml";
import yaml from "highlight.js/lib/languages/yaml";
import type { Entry } from "../../store/thread";

for (const [name, lang] of Object.entries({ bash, css, diff, go, javascript, json, python, rust, sql, swift, typescript, xml, yaml })) {
  hljs.registerLanguage(name, lang);
}
hljs.registerAliases(["sh", "zsh", "shell", "console"], { languageName: "bash" });
hljs.registerAliases(["js", "jsx", "mjs"], { languageName: "javascript" });
hljs.registerAliases(["ts", "tsx"], { languageName: "typescript" });
hljs.registerAliases(["html", "svg"], { languageName: "xml" });
hljs.registerAliases(["yml"], { languageName: "yaml" });
hljs.registerAliases(["py"], { languageName: "python" });

export function escapeHTML(s: string) {
  return s.replace(/[&<>"]/g, (c) => (c === "&" ? "&amp;" : c === "<" ? "&lt;" : c === ">" ? "&gt;" : "&quot;"));
}

// While a block streams, its code isn't highlighted: it would be done again on every frame.
let highlighting = true;

const marked = new Marked({
  gfm: true,
  breaks: false,
  renderer: {
    code({ text, lang }) {
      const language = (lang ?? "").split(/\s/)[0].toLowerCase();
      let body: string;
      if (highlighting && language && hljs.getLanguage(language) && text.length < 40_000) {
        body = hljs.highlight(text, { language, ignoreIllegals: true }).value;
      } else {
        body = escapeHTML(text);
      }
      const label = language ? `<span class="lang">${escapeHTML(language)}</span>` : "";
      return `<div class="code">${label}<pre><code>${body}</code></pre></div>`;
    },
    html({ text }) {
      // Claude's HTML is shown, not run.
      return escapeHTML(text);
    },
    link({ href, tokens }) {
      const text = this.parser.parseInline(tokens);
      return `<a href="${escapeHTML(href)}" target="_blank" rel="noreferrer">${text}</a>`;
    },
  },
});

export function renderMarkdown(text: string, live: boolean): string {
  highlighting = !live;
  try {
    return marked.parse(text, { async: false }) as string;
  } catch {
    return `<p>${escapeHTML(text)}</p>`;
  } finally {
    highlighting = true;
  }
}

/** An entry's markdown, rendered once per change of its text. */
export function entryHTML(e: Entry): string {
  if (document.documentElement.classList.contains("no-md")) return escapeHTML(e.text);
  const stamp = e.text.length * 2 + (e.live ? 1 : 0);
  if (e.html === undefined || e.htmlFor !== stamp) {
    e.html = renderMarkdown(e.text, e.live);
    e.htmlFor = stamp;
  }
  return e.html;
}
