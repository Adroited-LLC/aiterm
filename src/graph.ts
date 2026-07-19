import { CommitInfo } from "./ipc";

/** One rendered row of the commit graph. */
export interface GraphRow {
  commit: CommitInfo;
  /** Lane (column) of this commit's dot. */
  lane: number;
  /** Lines entering from the row above: [fromLane, toLane] pairs where
   *  toLane is where the line lands in this row (dot lane for merges). */
  inputs: Array<[number, number]>;
  /** Lines leaving to the row below: [fromLane, toLane]; fromLane is the dot
   *  lane for this commit's parents, or a pass-through lane. */
  outputs: Array<[number, number]>;
  maxLane: number;
}

/**
 * Classic lane assignment: each active lane carries the commit id it expects
 * next (its parent chain). Commits claim the leftmost lane expecting them;
 * merges close extra lanes; extra parents open or join lanes to the right.
 */
export function computeGraph(commits: CommitInfo[]): GraphRow[] {
  const lanes: (string | null)[] = [];
  const rows: GraphRow[] = [];

  const alloc = (id: string): number => {
    const free = lanes.indexOf(null);
    if (free !== -1) {
      lanes[free] = id;
      return free;
    }
    lanes.push(id);
    return lanes.length - 1;
  };

  for (const c of commits) {
    const claiming: number[] = [];
    lanes.forEach((v, i) => {
      if (v === c.id) claiming.push(i);
    });

    const lane = claiming.length > 0 ? claiming[0] : alloc(c.id);
    const inputs: Array<[number, number]> = [];
    const outputs: Array<[number, number]> = [];

    // Pass-through lanes (not involved with this commit).
    lanes.forEach((v, i) => {
      if (v !== null && v !== c.id) {
        inputs.push([i, i]);
        outputs.push([i, i]);
      }
    });
    // Lines converging into the dot (branch tips only have none).
    if (rows.length > 0) {
      for (const i of claiming) inputs.push([i, lane]);
    }

    // Close the extra lanes that merged into this commit.
    for (const i of claiming.slice(1)) lanes[i] = null;

    // First parent continues in this lane; extra parents fan out.
    if (c.parents.length === 0) {
      lanes[lane] = null;
    } else {
      lanes[lane] = c.parents[0];
      outputs.push([lane, lane]);
      for (const p of c.parents.slice(1)) {
        const existing = lanes.findIndex((v, i) => v === p && i !== lane);
        outputs.push([lane, existing !== -1 ? existing : alloc(p)]);
      }
    }

    while (lanes.length > 0 && lanes[lanes.length - 1] === null) lanes.pop();

    rows.push({
      commit: c,
      lane,
      inputs,
      outputs,
      maxLane: Math.max(
        lane,
        ...inputs.flat(),
        ...outputs.flat(),
        lanes.length - 1,
      ),
    });
  }
  return rows;
}

export const LANE_COLORS = [
  "#da7756", "#61afef", "#98c379", "#c678dd", "#e5c07b", "#56b6c2", "#e06c75",
];

export const laneColor = (lane: number) => LANE_COLORS[lane % LANE_COLORS.length];
