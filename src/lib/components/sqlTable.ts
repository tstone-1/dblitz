// Extract the primary (first top-level) table name from a SQL query so the
// Execute SQL grid can reuse the column colors saved for that table in Browse
// Data. This is a deliberately lightweight scanner, not a full SQL parser:
// it finds the first `FROM` keyword at paren-depth 0 (so subquery and CTE-body
// FROMs are skipped) and returns the table identifier that follows. A `FROM (`
// subquery, a missing FROM, or anything it can't read yields `null`, in which
// case the caller simply renders the result without colors.

interface NamePart {
  name: string;
  end: number;
}

const WORD_START = /[A-Za-z_]/;
const WORD_CHAR = /[A-Za-z0-9_$]/;

// Strip `-- line` and `/* block */` comments so keywords inside them never
// trip the scanner. Quoted spans (string literals, "double"/`back`/[bracket]
// identifiers) are copied verbatim so a `--` or `/*` *inside* them is not
// mistaken for a comment — otherwise a query like `SELECT '--' FROM t` would
// lose its closing context and the FROM table would go undetected.
function stripSqlComments(sql: string): string {
  let out = "";
  let i = 0;
  const n = sql.length;

  while (i < n) {
    const c = sql[i];

    // Quoted spans: copy through to the matching close, untouched.
    if (c === "'" || c === '"' || c === "`") {
      const close = c;
      out += c;
      i++;
      while (i < n) {
        out += sql[i];
        if (sql[i] === close) {
          // Doubled quote is an escaped literal — stay inside the span.
          if (sql[i + 1] === close) {
            out += sql[i + 1];
            i += 2;
            continue;
          }
          i++;
          break;
        }
        i++;
      }
      continue;
    }
    if (c === "[") {
      out += c;
      i++;
      while (i < n) {
        out += sql[i];
        if (sql[i] === "]") {
          i++;
          break;
        }
        i++;
      }
      continue;
    }

    // Line comment -- ... \n
    if (c === "-" && sql[i + 1] === "-") {
      i += 2;
      while (i < n && sql[i] !== "\n") i++;
      out += " ";
      continue;
    }

    // Block comment /* ... */
    if (c === "/" && sql[i + 1] === "*") {
      i += 2;
      while (i < n && !(sql[i] === "*" && sql[i + 1] === "/")) i++;
      i += 2;
      out += " ";
      continue;
    }

    out += c;
    i++;
  }

  return out;
}

// Read a single identifier segment at position `i`: a bare word, a
// double-quoted / backtick-quoted name, or a `[bracketed]` name. Returns the
// unquoted text and the index just past it, or null if `i` is not on a name.
function readNamePart(s: string, i: number): NamePart | null {
  const c = s[i];

  if (c === '"' || c === "`") {
    const close = c;
    let j = i + 1;
    let out = "";
    while (j < s.length) {
      if (s[j] === close) {
        if (s[j + 1] === close) {
          // Doubled quote is an escaped literal quote inside the identifier.
          out += close;
          j += 2;
          continue;
        }
        j++;
        break;
      }
      out += s[j];
      j++;
    }
    return { name: out, end: j };
  }

  if (c === "[") {
    let j = i + 1;
    let out = "";
    while (j < s.length && s[j] !== "]") {
      out += s[j];
      j++;
    }
    return { name: out, end: j + 1 };
  }

  if (WORD_START.test(c)) {
    let j = i + 1;
    while (j < s.length && WORD_CHAR.test(s[j])) j++;
    return { name: s.slice(i, j), end: j };
  }

  return null;
}

// Parse the table reference starting at/after `pos`. Handles leading
// whitespace, schema-qualified `schema.table` (the last segment wins), and
// returns null for a `(` subquery.
function parseTableToken(s: string, pos: number): string | null {
  let i = pos;
  while (i < s.length && /\s/.test(s[i])) i++;
  if (i >= s.length || s[i] === "(") return null;

  const first = readNamePart(s, i);
  if (!first) return null;

  let name = first.name;
  let next = first.end;
  // schema.table (or db.schema.table) — keep walking dotted segments so the
  // final segment, the table name, is what we return.
  while (s[next] === ".") {
    const seg = readNamePart(s, next + 1);
    if (!seg) break;
    name = seg.name;
    next = seg.end;
  }

  return name || null;
}

/**
 * Return the first top-level table name referenced by `sql`, or null if none
 * can be confidently identified (no FROM, a subquery in the FROM slot, etc.).
 */
export function primaryTableFromSql(sql: string): string | null {
  const s = stripSqlComments(sql);
  let depth = 0;
  let i = 0;

  while (i < s.length) {
    const c = s[i];

    // Single-quoted string literal — skip wholesale (handles '' escape).
    if (c === "'") {
      i++;
      while (i < s.length) {
        if (s[i] === "'") {
          if (s[i + 1] === "'") {
            i += 2;
            continue;
          }
          i++;
          break;
        }
        i++;
      }
      continue;
    }

    // Quoted identifiers: skip so a name like "from" isn't read as the keyword.
    if (c === '"' || c === "`" || c === "[") {
      const part = readNamePart(s, i);
      i = part ? part.end : i + 1;
      continue;
    }

    if (c === "(") {
      depth++;
      i++;
      continue;
    }
    if (c === ")") {
      depth--;
      i++;
      continue;
    }

    if (WORD_START.test(c)) {
      let j = i + 1;
      while (j < s.length && WORD_CHAR.test(s[j])) j++;
      const word = s.slice(i, j);
      if (depth === 0 && word.toLowerCase() === "from") {
        return parseTableToken(s, j);
      }
      i = j;
      continue;
    }

    i++;
  }

  return null;
}

/**
 * Build the column-color map for an Execute SQL result by reusing the colors
 * saved (in Browse Data) for the query's primary FROM table. Pure and
 * dependency-free so it can be unit-tested apart from the Svelte component.
 *
 * Only result columns whose names match a colored column in that table are
 * colored; joins/aliases/expressions stay uncolored. Caveat: matching is by
 * name, so an aliased expression that reuses a real column name — e.g.
 * `SELECT SUM(qty) AS BusinessLine FROM db` — will inherit that column's color.
 * That is cosmetic and accepted for the "match the FROM table" approach.
 */
export function resolveResultColumnColors(params: {
  sql: string;
  columns: string[];
  tableNames: string[];
  getColumnColors: (table: string) => Record<string, string>;
}): Record<string, string> {
  const { sql, columns, tableNames, getColumnColors } = params;
  if (columns.length === 0) return {};

  const parsed = primaryTableFromSql(sql);
  if (!parsed) return {};

  // SQL table names are case-insensitive; configs are keyed by the real name,
  // so resolve the parsed name to the canonical table name.
  const canonical =
    tableNames.find((t) => t.toLowerCase() === parsed.toLowerCase()) ?? parsed;

  const saved = getColumnColors(canonical);
  const map: Record<string, string> = {};
  for (const col of columns) {
    if (saved[col]) map[col] = saved[col];
  }
  return map;
}
