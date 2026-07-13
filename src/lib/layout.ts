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
};

export type LayoutPart = {
  id: string;
  reference: string;
  value?: string;
  kind?: "generic" | "ic" | "axial";
  pins: LayoutPin[];
  color?: string;
};

export type LayoutWire = {
  id: string;
  from: BreadboardHole;
  to: BreadboardHole;
  color?: string;
  /** Air wires describe unrouted nets; routed wires describe the final result. */
  kind?: "air" | "routed";
};

export type LayoutFrame = {
  parts: LayoutPart[];
  wires?: LayoutWire[];
  iteration?: number;
  cost?: number;
};

export type ComputePhase = "idle" | "spectral" | "annealing" | "routing" | "done" | "error";

export type ComputeRequest = {
  profile: "quick" | "standard" | "full";
};

export type ComputeProgressEvent = {
  run_id: string | number;
  phase: Exclude<ComputePhase, "idle">;
  progress: number;
  message: string;
  frame?: LayoutFrame | null;
};
