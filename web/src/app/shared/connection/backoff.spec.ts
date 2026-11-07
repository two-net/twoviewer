import { MAX_DELAY_MS, reconnectDelay } from './backoff';

describe('reconnectDelay', () => {
  it('is randomized within the upper half of its ceiling', () => {
    expect(reconnectDelay(0, () => 0)).toBe(500);
    expect(reconnectDelay(0, () => 0.999_999)).toBeCloseTo(1000, 0);
    expect(reconnectDelay(2, () => 0.5)).toBe(3000);
  });

  it('increases with every attempt, up to the maximum', () => {
    const ceilings = [0, 1, 2, 3, 4, 5, 6, 7].map((attempt) => reconnectDelay(attempt, () => 1));

    expect(ceilings).toEqual([1000, 2000, 4000, 8000, 16_000, 30_000, 30_000, 30_000]);
    expect(reconnectDelay(10_000, () => 0)).toBe(MAX_DELAY_MS / 2);
  });
});
