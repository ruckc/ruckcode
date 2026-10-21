import DOMPurify from 'dompurify';
import { Marked } from 'marked';

const marked = new Marked({ gfm: true, breaks: false });

let hooked = false;
function ensureHook() {
  if (hooked) return;
  hooked = true;
  // Links open outside the app and never get a handle on it.
  DOMPurify.addHook('afterSanitizeAttributes', (node) => {
    if (node.tagName === 'A') {
      node.setAttribute('target', '_blank');
      node.setAttribute('rel', 'noopener noreferrer');
    }
  });
}

/** Render markdown to sanitized HTML. Safe to use with `{@html}`. */
export function renderMarkdown(src: string): string {
  ensureHook();
  const html = marked.parse(src, { async: false });
  return DOMPurify.sanitize(html, { USE_PROFILES: { html: true } });
}
