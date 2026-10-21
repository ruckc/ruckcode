/** Glob helpers for the editor's live previews (an approximation of the `globset` crate). */

/** Why a glob would be rejected, or null when it looks fine. */
export function globProblem(g: string): string | null {
  if (!g.trim()) return 'empty pattern';
  let depth = 0;
  let inClass = false;
  for (let i = 0; i < g.length; i++) {
    const c = g[i];
    if (c === '\\') {
      i++;
      continue;
    }
    if (inClass) {
      if (c === ']' && g[i - 1] !== '[' && !(g[i - 1] === '!' && g[i - 2] === '[')) inClass = false;
      continue;
    }
    if (c === '[') inClass = true;
    else if (c === '{') depth++;
    else if (c === '}') {
      if (depth === 0) return 'unmatched `}`';
      depth--;
    }
  }
  if (inClass) return 'unclosed `[`';
  if (depth > 0) return 'unclosed `{`';
  if (/[^/*]\*\*|\*\*[^/*]/.test(g)) return '`**` must be a whole path component';
  return null;
}

/** Anchored RegExp for a glob (`*` also matches `:` and `/`, as for task and tool names). */
export function globToRegExp(g: string): RegExp | null {
  if (globProblem(g)) return null;
  let out = '';
  let depth = 0;
  for (let i = 0; i < g.length; i++) {
    const c = g[i];
    if (c === '\\') {
      out += '\\' + (g[++i] ?? '\\');
    } else if (c === '*') {
      while (g[i + 1] === '*') i++;
      out += '.*';
    } else if (c === '?') out += '.';
    else if (c === '[') {
      const end = g.indexOf(']', i + 2);
      let cls = g.slice(i + 1, end).replace(/\\/g, '\\\\');
      if (cls.startsWith('!')) cls = '^' + cls.slice(1);
      out += `[${cls}]`;
      i = end;
    } else if (c === '{') {
      depth++;
      out += '(?:';
    } else if (c === '}') {
      depth--;
      out += ')';
    } else if (c === ',' && depth > 0) out += '|';
    else out += c.replace(/[.+^$()|\\]/g, '\\$&');
  }
  try {
    return new RegExp(`^${out}$`);
  } catch {
    return null;
  }
}

/** Names matched by a glob; null when the glob is invalid. */
export function globMatches(g: string, names: string[]): string[] | null {
  const re = globToRegExp(g);
  return re ? names.filter((n) => re.test(n)) : null;
}

/** Message for an invalid hook matcher (anchored like the agent: `^(?:m)$`), or null. */
export function matcherProblem(m: string): string | null {
  if (/\(\?<?[=!]/.test(m)) return 'lookaround is not supported';
  try {
    new RegExp(`^(?:${m})$`);
    return null;
  } catch (e) {
    const msg = e instanceof Error ? e.message : String(e);
    return msg.replace(/^Invalid regular expression: /, '').replace(/^\/.*\/[a-z]*: /, '');
  }
}
