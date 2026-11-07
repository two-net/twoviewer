import { Provider } from '@angular/core';
import { ClientMessage, ServerMessage } from './messages';
import { Socket, WEB_SOCKET } from './server-connection';

/** A WebSocket for tests, at whose other end the test plays the server. */
export class FakeSocket implements Socket {
  readyState = 1;
  onmessage: ((event: MessageEvent) => void) | null = null;
  onclose: ((event: CloseEvent) => void) | null = null;
  /** What the client sent. */
  readonly sent: ClientMessage[] = [];
  /** Whether the client closed it. */
  closed = false;

  constructor(readonly url: string) {}

  send(data: string): void {
    this.sent.push(JSON.parse(data));
  }

  close(): void {
    this.closed = true;
    this.readyState = 3;
  }

  /** The server sends `message`. */
  receive(message: ServerMessage | { readonly type: string }): void {
    this.onmessage?.({ data: JSON.stringify(message) } as MessageEvent);
  }

  /** The connection ends without the client closing it. */
  drop(): void {
    this.readyState = 3;
    this.onclose?.({} as CloseEvent);
  }
}

/** The sockets the client opened, oldest first. */
export class FakeSockets {
  readonly opened: FakeSocket[] = [];

  readonly open = (url: string): FakeSocket => {
    const socket = new FakeSocket(url);
    this.opened.push(socket);
    return socket;
  };

  get last(): FakeSocket {
    return this.opened[this.opened.length - 1];
  }
}

/** Replaces the WebSockets of the server connection with fakes. */
export function provideFakeSockets(sockets = new FakeSockets()): Provider {
  return { provide: WEB_SOCKET, useValue: sockets.open };
}
