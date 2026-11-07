import { DOCUMENT } from '@angular/common';
import { DestroyRef, Injectable, InjectionToken, inject, signal } from '@angular/core';
import { Observable, Subject } from 'rxjs';
import { reconnectDelay } from './backoff';
import {
  ClientMessage,
  NOT_SIGNED_IN,
  REALTIME_PATH,
  ServerEvent,
  ServerMessage,
} from './messages';

/** How long opening a connection may take, until the server's hello. */
export const CONNECT_TIMEOUT_MS = 10_000;

/**
 * How long the connection may be down before the banner appears, so that a server restart, which
 * takes a reconnection of about a second, does not flash it.
 */
export const LOST_AFTER_MS = 2000;

/** `WebSocket.OPEN`, which test environments may not define. */
const OPEN = 1;

/** The part of a WebSocket that the connection uses. */
export interface Socket {
  readonly readyState: number;
  onmessage: ((event: MessageEvent) => void) | null;
  onclose: ((event: CloseEvent) => void) | null;
  send(data: string): void;
  close(): void;
}

/** Opens WebSockets. Tests replace it with fakes. */
export const WEB_SOCKET = new InjectionToken<(url: string) => Socket>('WEB_SOCKET', {
  providedIn: 'root',
  factory: () => (url) => new WebSocket(url),
});

/**
 * - `idle`: not started.
 * - `connecting`: a connection is being opened.
 * - `connected`: the server said hello.
 * - `reconnecting`: waiting before the next attempt.
 */
export type ConnectionState = 'idle' | 'connecting' | 'connected' | 'reconnecting';

/**
 * Why the server closed the connection for want of a sign-in:
 * - `ended`: the sign-in was active while connected, and has been revoked or has expired.
 * - `none`: the browser had no active sign-in when it connected.
 */
export type SignedOutReason = 'ended' | 'none';

/**
 * The real-time connection to the server (IF-09), which keeps screens up to date without a
 * reload (UI-02). The messages are described in `protocol/src/lib.rs`.
 *
 * Features show what {@link events} says has changed, and fetch everything they show again on
 * every {@link resyncs}: after every connection, and whenever events may have been missed.
 *
 * After every interruption it reconnects with randomized, increasing delays (REL-05), and at once
 * when the browser is back online or the tab is shown again. While it has been down for longer
 * than {@link LOST_AFTER_MS}, {@link lost} is true and `<app-connection-banner>` says so (UI-05).
 *
 * The server accepts it only with an active sign-in. When the browser has none, or its sign-in
 * ends, the server closes it with {@link NOT_SIGNED_IN}: the connection stops, and
 * {@link signedOut} says why.
 */
@Injectable({ providedIn: 'root' })
export class ServerConnection {
  private readonly document = inject(DOCUMENT);
  private readonly openSocket = inject(WEB_SOCKET);

  private readonly current = signal<ConnectionState>('idle');
  readonly state = this.current.asReadonly();
  private readonly down = signal(false);
  /** Whether the connection has been down long enough to tell the user. */
  readonly lost = this.down.asReadonly();

  private readonly eventSubject = new Subject<ServerEvent>();
  readonly events: Observable<ServerEvent> = this.eventSubject.asObservable();
  private readonly resyncSubject = new Subject<void>();
  readonly resyncs: Observable<void> = this.resyncSubject.asObservable();
  private readonly signedOutSubject = new Subject<SignedOutReason>();
  /** Fires when the server says that this browser's sign-in has ended, or that it has none. */
  readonly signedOut: Observable<SignedOutReason> = this.signedOutSubject.asObservable();

  private socket?: Socket;
  private attempt = 0;
  /** How long the server may be silent. Until its hello, the connect timeout. */
  private silenceMs = CONNECT_TIMEOUT_MS;
  private retryTimer?: ReturnType<typeof setTimeout>;
  private silenceTimer?: ReturnType<typeof setTimeout>;
  private lostTimer?: ReturnType<typeof setTimeout>;

  private readonly onOnline = () => this.retryNow(true);
  private readonly onOffline = () => this.dropNow();
  private readonly onVisibilityChange = () => {
    if (this.document.visibilityState === 'visible') {
      this.retryNow(false);
    }
  };

  constructor() {
    inject(DestroyRef).onDestroy(() => this.stop());
  }

  /** Opens the connection and keeps it open, until {@link stop}. Does nothing if started. */
  start(): void {
    if (this.current() !== 'idle') {
      return;
    }
    const window = this.document.defaultView;
    window?.addEventListener('online', this.onOnline);
    window?.addEventListener('offline', this.onOffline);
    this.document.addEventListener('visibilitychange', this.onVisibilityChange);
    this.watchForLoss();
    this.connect();
  }

  /** Closes the connection and stops reconnecting. */
  stop(): void {
    const window = this.document.defaultView;
    window?.removeEventListener('online', this.onOnline);
    window?.removeEventListener('offline', this.onOffline);
    this.document.removeEventListener('visibilitychange', this.onVisibilityChange);
    this.closeSocket();
    clearTimeout(this.retryTimer);
    clearTimeout(this.lostTimer);
    this.lostTimer = undefined;
    this.attempt = 0;
    this.down.set(false);
    this.current.set('idle');
  }

  private connect(): void {
    this.current.set('connecting');
    this.silenceMs = CONNECT_TIMEOUT_MS;
    let socket: Socket;
    try {
      socket = this.openSocket(this.url());
    } catch {
      this.reconnectLater();
      return;
    }
    this.socket = socket;
    socket.onmessage = (event) => this.receive(event.data);
    socket.onclose = (event) => {
      if (event.code === NOT_SIGNED_IN) {
        // Only an active sign-in gets a hello.
        const reason = this.current() === 'connected' ? 'ended' : 'none';
        this.stop();
        this.signedOutSubject.next(reason);
      } else {
        this.reconnectLater();
      }
    };
    this.expectMessage();
  }

  private url(): string {
    const { protocol, host } = this.document.location;
    return `${protocol === 'https:' ? 'wss:' : 'ws:'}//${host}${REALTIME_PATH}`;
  }

  private receive(data: unknown): void {
    let message: ServerMessage;
    try {
      message = JSON.parse(String(data));
    } catch {
      return;
    }
    // Anything from the server shows that it is still there.
    this.expectMessage();
    // Message types this version does not know are ignored.
    switch (message.type) {
      case 'hello':
        this.attempt = 0;
        this.silenceMs = message.heartbeatTimeoutMs;
        this.expectMessage();
        this.current.set('connected');
        clearTimeout(this.lostTimer);
        this.lostTimer = undefined;
        this.down.set(false);
        this.resyncSubject.next();
        break;
      case 'heartbeat':
        this.send({ type: 'heartbeat' });
        break;
      case 'resync':
        this.resyncSubject.next();
        break;
      case 'event':
        this.eventSubject.next({ name: message.name, data: message.data });
        break;
    }
  }

  private send(message: ClientMessage): void {
    if (this.socket?.readyState === OPEN) {
      this.socket.send(JSON.stringify(message));
    }
  }

  /** Treats the connection as lost unless a message arrives within the silence timeout. */
  private expectMessage(): void {
    clearTimeout(this.silenceTimer);
    this.silenceTimer = setTimeout(() => this.reconnectLater(), this.silenceMs);
  }

  /** Closes the connection, if any, and waits before the next attempt. */
  private reconnectLater(): void {
    this.closeSocket();
    this.watchForLoss();
    this.current.set('reconnecting');
    clearTimeout(this.retryTimer);
    this.retryTimer = setTimeout(() => this.connect(), reconnectDelay(this.attempt));
    this.attempt++;
  }

  /** Reconnects now, if waiting to. */
  private retryNow(resetDelays: boolean): void {
    if (resetDelays) {
      this.attempt = 0;
    }
    if (this.current() === 'reconnecting') {
      clearTimeout(this.retryTimer);
      this.connect();
    }
  }

  /** The browser is offline: the connection is lost, whatever the socket says. */
  private dropNow(): void {
    if (this.current() === 'idle') {
      return;
    }
    clearTimeout(this.lostTimer);
    this.lostTimer = undefined;
    this.down.set(true);
    if (this.current() !== 'reconnecting') {
      this.reconnectLater();
    }
  }

  /** Starts the time after which the user is told, unless it already runs or they know. */
  private watchForLoss(): void {
    if (this.lostTimer === undefined && !this.down()) {
      this.lostTimer = setTimeout(() => {
        this.lostTimer = undefined;
        this.down.set(true);
      }, LOST_AFTER_MS);
    }
  }

  private closeSocket(): void {
    clearTimeout(this.silenceTimer);
    const socket = this.socket;
    this.socket = undefined;
    if (socket) {
      socket.onmessage = null;
      socket.onclose = null;
      socket.close();
    }
  }
}
