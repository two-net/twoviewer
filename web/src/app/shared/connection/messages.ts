/**
 * The messages of the real-time connection, copied from `protocol/src/lib.rs`, which describes
 * the protocol. Keep the two in step.
 */

/** Where browsers open the connection. */
export const REALTIME_PATH = '/api/realtime';

/** A change to show. `data` identifies what changed; fetch the new state through the API. */
export interface ServerEvent {
  /** What happened, as `<feature>.<change>`, such as `device.status`. */
  readonly name: string;
  readonly data: unknown;
}

export type ServerMessage =
  /** The first message on every connection. */
  | { readonly type: 'hello'; readonly heartbeatTimeoutMs: number }
  /** Answer it with a heartbeat. */
  | { readonly type: 'heartbeat' }
  /** Events may have been missed: fetch the current state. */
  | { readonly type: 'resync' }
  | ({ readonly type: 'event' } & ServerEvent);

export type ClientMessage = { readonly type: 'heartbeat' };
