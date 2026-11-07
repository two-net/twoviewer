import { TestBed } from '@angular/core/testing';
import { ConnectionBanner } from './connection-banner';
import { NOT_SIGNED_IN, ServerEvent } from './messages';
import {
  CONNECT_TIMEOUT_MS,
  LOST_AFTER_MS,
  ServerConnection,
  SignedOutReason,
} from './server-connection';
import { FakeSocket, FakeSockets, provideFakeSockets } from './testing';

const HELLO = { type: 'hello', heartbeatTimeoutMs: 8000 } as const;

describe('ServerConnection', () => {
  let sockets: FakeSockets;
  let connection: ServerConnection;
  let events: ServerEvent[];
  let resyncs: number;

  beforeEach(() => {
    vi.useFakeTimers();
    // The shortest delays: half of each ceiling.
    vi.spyOn(Math, 'random').mockReturnValue(0);
    sockets = new FakeSockets();
    TestBed.configureTestingModule({ providers: [provideFakeSockets(sockets)] });
    connection = TestBed.inject(ServerConnection);
    events = [];
    resyncs = 0;
    connection.events.subscribe((event) => events.push(event));
    connection.resyncs.subscribe(() => resyncs++);
  });

  afterEach(() => {
    vi.useRealTimers();
    vi.restoreAllMocks();
  });

  /** Starts the connection and lets the server say hello. */
  function connected(): FakeSocket {
    connection.start();
    sockets.last.receive(HELLO);
    return sockets.last;
  }

  it('opens the WebSocket of this host and is connected once the server says hello', () => {
    expect(connection.state()).toBe('idle');

    connection.start();

    const { protocol, host } = document.location;
    const scheme = protocol === 'https:' ? 'wss:' : 'ws:';
    expect(sockets.opened.map((socket) => socket.url)).toEqual([`${scheme}//${host}/api/realtime`]);
    expect(connection.state()).toBe('connecting');

    sockets.last.receive(HELLO);
    expect(connection.state()).toBe('connected');
    // Every hello means that events may have been missed.
    expect(resyncs).toBe(1);
  });

  it('starts only once', () => {
    connection.start();
    connection.start();

    expect(sockets.opened.length).toBe(1);
  });

  it('answers heartbeats', () => {
    const socket = connected();

    socket.receive({ type: 'heartbeat' });

    expect(socket.sent).toEqual([{ type: 'heartbeat' }]);
  });

  it('passes on events and resyncs, and ignores messages it does not know', () => {
    const socket = connected();

    socket.receive({ type: 'event', name: 'device.status', data: { deviceId: '1' } });
    socket.receive({ type: 'resync' });
    socket.receive({ type: 'somethingNew' });

    expect(events).toEqual([{ name: 'device.status', data: { deviceId: '1' } }]);
    expect(resyncs).toBe(2);
    expect(connection.state()).toBe('connected');
  });

  it('reconnects after a randomized delay that increases with every failed attempt', () => {
    connected().drop();
    expect(connection.state()).toBe('reconnecting');

    // With Math.random() at 0, half of 1, 2 and 4 seconds.
    for (const delay of [500, 1000, 2000]) {
      const opened = sockets.opened.length;
      vi.advanceTimersByTime(delay - 1);
      expect(sockets.opened.length).toBe(opened);
      vi.advanceTimersByTime(1);
      expect(sockets.opened.length).toBe(opened + 1);
      expect(connection.state()).toBe('connecting');
      sockets.last.drop();
    }
  });

  it('starts the delays over once connected again', () => {
    connected().drop();
    vi.advanceTimersByTime(500);
    sockets.last.drop();
    vi.advanceTimersByTime(1000);
    sockets.last.receive(HELLO);

    sockets.last.drop();
    vi.advanceTimersByTime(500);

    expect(sockets.opened.length).toBe(4);
  });

  it('reconnects when the server is silent for longer than its heartbeat timeout', () => {
    const socket = connected();

    vi.advanceTimersByTime(7999);
    socket.receive({ type: 'heartbeat' });
    vi.advanceTimersByTime(7999);
    expect(socket.closed).toBe(false);
    vi.advanceTimersByTime(1);

    expect(socket.closed).toBe(true);
    expect(connection.state()).toBe('reconnecting');
  });

  it('gives up on a connection without a hello after the connect timeout', () => {
    connection.start();

    vi.advanceTimersByTime(CONNECT_TIMEOUT_MS);

    expect(sockets.last.closed).toBe(true);
    expect(connection.state()).toBe('reconnecting');
  });

  it('reports the connection as lost only once it has been down for a while', () => {
    connected().drop();

    // A quick reconnection, such as after a server restart, goes unnoticed.
    vi.advanceTimersByTime(500);
    sockets.last.receive(HELLO);
    vi.advanceTimersByTime(LOST_AFTER_MS);
    expect(connection.lost()).toBe(false);

    sockets.last.drop();
    vi.advanceTimersByTime(LOST_AFTER_MS - 1);
    expect(connection.lost()).toBe(false);
    vi.advanceTimersByTime(1);
    expect(connection.lost()).toBe(true);

    vi.advanceTimersByTime(30_000);
    sockets.last.receive(HELLO);
    expect(connection.lost()).toBe(false);
  });

  it('reports a first connection that does not succeed as lost too', () => {
    connection.start();

    vi.advanceTimersByTime(LOST_AFTER_MS);

    expect(connection.lost()).toBe(true);
  });

  it('is lost at once when the browser goes offline, and reconnects at once when it is back', () => {
    const socket = connected();

    window.dispatchEvent(new Event('offline'));
    expect(connection.lost()).toBe(true);
    expect(socket.closed).toBe(true);
    expect(connection.state()).toBe('reconnecting');

    // Attempts fail while offline, and the delays grow.
    vi.advanceTimersByTime(500);
    sockets.last.drop();
    vi.advanceTimersByTime(1000);
    sockets.last.drop();
    const opened = sockets.opened.length;
    window.dispatchEvent(new Event('online'));
    expect(sockets.opened.length).toBe(opened + 1);

    // The delays start over.
    sockets.last.drop();
    vi.advanceTimersByTime(500);
    expect(sockets.opened.length).toBe(opened + 2);
  });

  it('reconnects at once when the tab is shown again', () => {
    vi.spyOn(document, 'visibilityState', 'get').mockReturnValue('visible');
    connected().drop();
    const opened = sockets.opened.length;

    document.dispatchEvent(new Event('visibilitychange'));

    expect(sockets.opened.length).toBe(opened + 1);
  });

  it('closes the connection and stops reconnecting when stopped', () => {
    const socket = connected();

    connection.stop();
    socket.drop();
    vi.advanceTimersByTime(60_000);

    expect(socket.closed).toBe(true);
    expect(sockets.opened.length).toBe(1);
    expect(connection.state()).toBe('idle');
    expect(connection.lost()).toBe(false);
  });

  it('stops and reports it when the server says that the sign-in has ended', () => {
    const reasons: SignedOutReason[] = [];
    connection.signedOut.subscribe((reason) => reasons.push(reason));
    const socket = connected();

    socket.drop(NOT_SIGNED_IN);
    vi.advanceTimersByTime(60_000);

    expect(reasons).toEqual(['ended']);
    // Reconnecting would only be closed again: the browser has to sign in first.
    expect(sockets.opened.length).toBe(1);
    expect(connection.state()).toBe('idle');
    expect(connection.lost()).toBe(false);

    // Started again once signed in.
    connected();
    expect(connection.state()).toBe('connected');
  });

  it('reports a browser that has no sign-in to start with', () => {
    const reasons: SignedOutReason[] = [];
    connection.signedOut.subscribe((reason) => reasons.push(reason));
    connection.start();

    // Before any hello.
    sockets.last.drop(NOT_SIGNED_IN);
    vi.advanceTimersByTime(60_000);

    expect(reasons).toEqual(['none']);
    expect(sockets.opened.length).toBe(1);
  });

  it('reconnects after any other close', () => {
    const reasons: SignedOutReason[] = [];
    connection.signedOut.subscribe((reason) => reasons.push(reason));
    const socket = connected();

    socket.drop(1012);
    vi.advanceTimersByTime(500);

    expect(sockets.opened.length).toBe(2);
    expect(reasons).toEqual([]);
  });
});

describe('ConnectionBanner', () => {
  let sockets: FakeSockets;

  beforeEach(() => {
    vi.useFakeTimers();
    sockets = new FakeSockets();
    TestBed.configureTestingModule({ providers: [provideFakeSockets(sockets)] });
  });

  afterEach(() => vi.useRealTimers());

  // detectChanges(), because whenStable() waits for timers, which are fake here.
  it('says in a live region that the connection is lost, until it is back', () => {
    const fixture = TestBed.createComponent(ConnectionBanner);
    const connection = TestBed.inject(ServerConnection);
    connection.start();
    sockets.last.receive(HELLO);
    fixture.detectChanges();
    const region: HTMLElement = fixture.nativeElement.querySelector('[role="status"]');
    expect(region.textContent?.trim()).toBe('');

    sockets.last.drop();
    vi.advanceTimersByTime(LOST_AFTER_MS);
    fixture.detectChanges();
    expect(region.textContent).toContain('Connection lost. Reconnecting…');

    vi.advanceTimersByTime(30_000);
    sockets.last.receive(HELLO);
    fixture.detectChanges();
    expect(region.textContent?.trim()).toBe('');
  });
});
