import { leadingZeroBits, puzzleInput, solve } from './proof-of-work-solver';

/** The example in `server/src/accounts/challenge.rs`, so that both sides agree on the puzzle. */
const EXAMPLE = { id: '6f1c2a7e-3b4d-4c5e-8f90-1a2b3c4d5e6f', difficulty: 12 };
/** The smallest nonce for the example. Its hash starts with 0002b5d8, 14 zero bits. */
const EXAMPLE_NONCE = 46;

async function sha256(data: Uint8Array<ArrayBuffer>): Promise<Uint8Array> {
  return new Uint8Array(await crypto.subtle.digest('SHA-256', data));
}

describe('proof of work', () => {
  it('hashes the challenge ID and the nonce as 8 big-endian bytes', () => {
    const input = puzzleInput(EXAMPLE.id, 0x0102);

    expect(Array.from(input.slice(0, 4))).toEqual([0x6f, 0x1c, 0x2a, 0x7e]);
    expect(Array.from(input.slice(12, 16))).toEqual([0x3c, 0x4d, 0x5e, 0x6f]);
    expect(Array.from(input.slice(16))).toEqual([0, 0, 0, 0, 0, 0, 1, 2]);
    expect(() => puzzleInput('not an ID', 0)).toThrow();
  });

  it('counts zero bits across bytes', () => {
    expect(leadingZeroBits(new Uint8Array([0xff]))).toBe(0);
    expect(leadingZeroBits(new Uint8Array([0x01, 0x00]))).toBe(7);
    expect(leadingZeroBits(new Uint8Array([0x00, 0x00, 0x10]))).toBe(19);
    expect(leadingZeroBits(new Uint8Array([0x00, 0x00]))).toBe(16);
  });

  it('agrees with the server on the example', async () => {
    const hash = await sha256(puzzleInput(EXAMPLE.id, EXAMPLE_NONCE));

    expect(Array.from(hash.slice(0, 4))).toEqual([0x00, 0x02, 0xb5, 0xd8]);
    expect(await solve(EXAMPLE)).toEqual({ id: EXAMPLE.id, nonce: EXAMPLE_NONCE });
  });

  it('solves any challenge at a low difficulty', async () => {
    const challenge = { id: crypto.randomUUID(), difficulty: 6 };

    const solution = await solve(challenge);

    expect(solution.id).toBe(challenge.id);
    expect(
      leadingZeroBits(await sha256(puzzleInput(challenge.id, solution.nonce))),
    ).toBeGreaterThanOrEqual(6);
  });
});
