export interface Calculation {
  input: number;
  doubled: number;
}

/** Double a number without changing the original input. */
export function double(value: number): number {
  return value * 2;
}

export function calculate(input: number): Calculation {
  return { input, doubled: double(input) };
}
