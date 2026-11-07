/** Client-side paging for long tables (10 000 devices, thousands of sessions): only one page is rendered. */
export const PAGE_SIZE = 100;

export interface Page<T> {
  /** 0-based, clamped to the existing pages. */
  page: number;
  pages: number;
  /** 1-based positions of the first and last row shown; 0, 0 when empty. */
  from: number;
  to: number;
  total: number;
  items: T[];
}

export function paginate<T>(rows: T[], page: number, size = PAGE_SIZE): Page<T> {
  const pages = Math.max(1, Math.ceil(rows.length / size));
  const p = Math.min(Math.max(0, Math.floor(page) || 0), pages - 1);
  const start = p * size;
  const items = rows.slice(start, start + size);
  return { page: p, pages, from: items.length ? start + 1 : 0, to: start + items.length, total: rows.length, items };
}
