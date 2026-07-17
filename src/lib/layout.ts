export type BreadboardPreset = "hole170" | "hole400" | "hole830";

export type BreadboardSelection = {
  preset: BreadboardPreset;
  boardCols: number;
  upperHalfOnly: boolean;
};

export type BreadboardRegion =
  | "main-top"
  | "main-bottom"
  | "rail-top"
  | "rail-bottom";

/** A logical hole. Rows are zero-based within their region. */
export type BreadboardHole = {
  region: BreadboardRegion;
  col: number;
  row: number;
};

export type LayoutPin = {
  hole: BreadboardHole;
  number?: string;
  name?: string;
  pin_type?: string;
  pin_shape?: string;
  unit?: number;
  net_id?: string;
  net_name?: string;
};

export type LayoutPart = {
  id: string;
  reference: string;
  value?: string;
  description?: string;
  datasheet?: string;
  footprint: string;
  package?: "generic" | "dip" | "axial";
  device?: "generic" | "diode" | "led";
  pins: LayoutPin[];
  properties?: Array<{
    name: string;
    value: string;
    hidden: boolean;
  }>;
  exclude_from_sim?: boolean;
  in_bom?: boolean;
  on_board?: boolean;
  in_pos_files?: boolean;
  dnp?: boolean;
  color?: string;
};

export type LayoutWire = {
  id: string;
  from: BreadboardHole;
  to: BreadboardHole;
  color?: string;
  /** Air wires are hints; rail ties are on-board links; rail links join physical boards. */
  kind?: "air" | "routed" | "rail-tie" | "rail-link";
  net_id?: string;
  net_name?: string;
};

export type CircuitSelection =
  | { type: "component"; id: string; label: string }
  | { type: "net"; id: string; label: string }
  | { type: "wire"; id: string; label: string; netId?: string };

export type LayoutFrame = {
  board_cols: number;
  board_count: number;
  gap_cols: number;
  total_cols: number;
  parts: LayoutPart[];
  wires?: LayoutWire[];
  iteration?: number;
  cost?: number;
};

export type ComputePhase = "idle" | "spectral" | "annealing" | "routing" | "done" | "error";

export type KiCadTextSegment = {
  text: string;
  overbar: boolean;
};

/** Parse KiCad's ~{text} markup without turning file content into HTML. */
export function parseKiCadTextMarkup(text: string): KiCadTextSegment[] {
  const segments: KiCadTextSegment[] = [];
  let cursor = 0;
  while (cursor < text.length) {
    const start = text.indexOf("~{", cursor);
    if (start < 0) {
      segments.push({ text: text.slice(cursor), overbar: false });
      break;
    }
    if (start > cursor) segments.push({ text: text.slice(cursor, start), overbar: false });
    let depth = 1;
    let closing = -1;
    for (let index = start + 2; index < text.length; index += 1) {
      if (text[index] === "{") depth += 1;
      else if (text[index] === "}") {
        depth -= 1;
        if (depth === 0) {
          closing = index;
          break;
        }
      }
    }
    if (closing < 0) {
      segments.push({ text: text.slice(start), overbar: false });
      break;
    }
    segments.push({ text: text.slice(start + 2, closing), overbar: true });
    cursor = closing + 1;
  }
  return segments.filter((segment) => segment.text.length > 0);
}

export type ComputeRequest = {
  profile: "quick" | "standard" | "full";
  locale: "zh-CN" | "en";
};

export type ComputeProgressEvent = {
  run_id: string | number;
  phase: Exclude<ComputePhase, "idle">;
  progress: number;
  message: string;
  frame?: LayoutFrame | null;
};
