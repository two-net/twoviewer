// The proof-of-work puzzle, without Angular, so that the worker that runs it stays small.

/**
 * A proof-of-work challenge, which the server asks for when a network signs up often (AUTH-15).
 * Solving one takes a second or two of computing, with no puzzle for people to solve and no
 * third-party service. The server's side is `server/src/accounts/challenge.rs`.
 */
export interface Challenge {
  readonly id: string;
  /** How many leading zero bits the hash of a solution needs. */
  readonly difficulty: number;
}

export interface Solution {
  readonly id: string;
  readonly nonce: number;
}

export type Digest = (data: Uint8Array<ArrayBuffer>) => Promise<ArrayBuffer>;

/** How many hashes are computed at once. WebCrypto is fastest with many in flight. */
const BATCH = 256;

const sha256: Digest = (data) => crypto.subtle.digest('SHA-256', data);

/**
 * What is hashed for `nonce`: the 16 bytes of the challenge ID, then the nonce as 8 big-endian
 * bytes.
 */
export function puzzleInput(id: string, nonce: number): Uint8Array<ArrayBuffer> {
  const hex = id.replaceAll('-', '');
  if (!/^[0-9a-f]{32}$/i.test(hex)) {
    throw new Error(`not a challenge ID: ${id}`);
  }
  const input = new Uint8Array(24);
  for (let i = 0; i < 16; i++) {
    input[i] = parseInt(hex.slice(2 * i, 2 * i + 2), 16);
  }
  new DataView(input.buffer).setBigUint64(16, BigInt(nonce));
  return input;
}

/** The number of zero bits at the start of `hash`. */
export function leadingZeroBits(hash: Uint8Array): number {
  let bits = 0;
  for (const byte of hash) {
    if (byte === 0) {
      bits += 8;
    } else {
      return bits + Math.clz32(byte) - 24;
    }
  }
  return bits;
}

/** Finds the smallest nonce that solves `challenge`. */
export async function solve(challenge: Challenge, digest: Digest = sha256): Promise<Solution> {
  for (let start = 0; start < Number.MAX_SAFE_INTEGER; start += BATCH) {
    const hashes = await Promise.all(
      Array.from({ length: BATCH }, (_, i) => digest(puzzleInput(challenge.id, start + i))),
    );
    const index = hashes.findIndex(
      (hash) => leadingZeroBits(new Uint8Array(hash)) >= challenge.difficulty,
    );
    if (index >= 0) {
      return { id: challenge.id, nonce: start + index };
    }
  }
  throw new Error('no solution');
}
