export type BreadboardPreset = "hole170" | "hole400" | "hole800";

export type BreadboardSelection = {
  preset: BreadboardPreset;
  cols: number;
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
  color?: string;
};

export type LayoutWire = {
  id: string;
  from: BreadboardHole;
  to: BreadboardHole;
  color?: string;
  /** Air wires are unrouted hints; rail ties are explicit fixed power-rail jumpers. */
  kind?: "air" | "routed" | "rail-tie";
  net_id?: string;
  net_name?: string;
};

export type CircuitSelection =
  | { type: "component"; id: string; label: string }
  | { type: "net"; id: string; label: string }
  | { type: "wire"; id: string; label: string; netId?: string };

export type LayoutFrame = {
  parts: LayoutPart[];
  wires?: LayoutWire[];
  iteration?: number;
  cost?: number;
};

export type ComputePhase = "idle" | "spectral" | "annealing" | "routing" | "done" | "error";

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
