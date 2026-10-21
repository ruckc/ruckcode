// @vitest-environment jsdom
import { describe, expect, it } from 'vitest';
import { renderMarkdown } from './markdown';

describe('renderMarkdown', () => {
  it('renders common markdown', () => {
    const html = renderMarkdown('# T\n\n**b** and `c`\n\n- x\n- y\n\n```ts\nlet a = 1;\n```');
    expect(html).toContain('<h1');
    expect(html).toContain('<strong>b</strong>');
    expect(html).toContain('<li>x</li>');
    expect(html).toContain('<pre><code class="language-ts">');
  });

  it('strips scripts, handlers and javascript: links', () => {
    const html = renderMarkdown(
      '<script>alert(1)</script><img src=x onerror=alert(1)>\n\n[x](javascript:alert(1))'
    );
    expect(html).not.toMatch(/<script/i);
    expect(html).not.toMatch(/onerror/i);
    expect(html).not.toMatch(/javascript:/i);
  });

  it('opens links in a new window without opener access', () => {
    const html = renderMarkdown('[x](https://example.com)');
    expect(html).toContain('target="_blank"');
    expect(html).toContain('rel="noopener noreferrer"');
  });

  it('copes with an unfinished code fence while streaming', () => {
    expect(renderMarkdown('```js\nconst a')).toContain('<pre><code');
  });
});
