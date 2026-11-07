/**
 * How long to wait before reconnecting (REL-05). The delays are randomized, so that clients that
 * lost the server at the same moment do not all come back at the same moment, and increasing, so
 * that a server that is down is not flooded. The Host Agent uses the same rule
 * (`agent/src/backoff.rs`).
 *
 * Constants for now: the web client gets no settings from the server yet (MNT-01).
 */

/** The longest possible delay before the first attempt. */
export const FIRST_DELAY_MS = 1000;
/** The longest possible delay before any attempt. */
export const MAX_DELAY_MS = 30_000;

/**
 * The delay before reconnection attempt `attempt`, counted from 0 since the last successful
 * connection. It is between half and all of `FIRST_DELAY_MS × 2^attempt`, at most
 * {@link MAX_DELAY_MS}.
 */
export function reconnectDelay(attempt: number, random: () => number = Math.random): number {
  const ceiling = Math.min(FIRST_DELAY_MS * 2 ** attempt, MAX_DELAY_MS);
  return ceiling * (0.5 + Math.min(Math.max(random(), 0), 1) / 2);
}
