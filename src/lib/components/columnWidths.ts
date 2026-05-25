interface TextMeasurer {
  /** Set before each measurement so header and cell widths use matching fonts. */
  font: string;
  measureText(text: string): { width: number };
}

interface ComputeAutoWidthsOptions {
  columns: string[];
  rows: (string | null)[][];
  getColumnIndex: (column: string) => number | undefined;
  measurer: TextMeasurer;
  maxSample?: number;
}

const CELL_PAD = 24; // 4px left + 8px right + border + buffer
const HEADER_EXTRA = 24; // sort arrow + pin glyph space
const MIN_WIDTH = 60;
const MAX_WIDTH = 400;
const MAX_SAMPLE = 100;
const HEADER_FONT = '600 12px "Cascadia Code","Cascadia Mono","Fira Code","Consolas",monospace';
const CELL_FONT = '12px "Cascadia Code","Cascadia Mono","Fira Code","Consolas",monospace';

export function computeAutoWidths({
  columns,
  rows,
  getColumnIndex,
  measurer,
  maxSample = MAX_SAMPLE,
}: ComputeAutoWidthsOptions): Record<string, number> {
  const widths: Record<string, number> = {};
  const rowCount = Math.min(rows.length, maxSample);

  for (const column of columns) {
    measurer.font = HEADER_FONT;
    let maxWidth = measurer.measureText(column).width + CELL_PAD + HEADER_EXTRA;

    measurer.font = CELL_FONT;
    const columnIndex = getColumnIndex(column);
    if (columnIndex !== undefined) {
      for (let index = 0; index < rowCount; index++) {
        const value = rows[index][columnIndex];
        if (value === null) {
          maxWidth = Math.max(maxWidth, measurer.measureText("NULL").width + CELL_PAD);
        } else if (value) {
          maxWidth = Math.max(maxWidth, measurer.measureText(value).width + CELL_PAD);
        }
      }
    }

    widths[column] = Math.round(Math.min(MAX_WIDTH, Math.max(MIN_WIDTH, maxWidth)));
  }

  return widths;
}
