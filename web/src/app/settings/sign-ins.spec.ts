import { provideHttpClient } from '@angular/common/http';
import { HttpTestingController, provideHttpClientTesting } from '@angular/common/http/testing';
import { ComponentFixture, TestBed } from '@angular/core/testing';
import { Router, provideRouter } from '@angular/router';
import { Confirmation } from '../shared/confirm/confirmation';
import { ServerConnection } from '../shared/connection/server-connection';
import { FakeSockets, provideFakeSockets } from '../shared/connection/testing';
import { Toasts } from '../shared/toast/toasts';
import { SignIn, SignIns } from './sign-ins';

const LAPTOP: SignIn = {
  id: '6f1c2d3e-0000-4000-8000-000000000001',
  client: 'Firefox on Linux',
  ipAddress: '203.0.113.7',
  signedInAt: '2026-11-01T09:00:00Z',
  lastActiveAt: '2026-11-08T12:00:00Z',
  current: true,
};
const PHONE: SignIn = {
  id: '6f1c2d3e-0000-4000-8000-000000000002',
  client: 'Safari on iOS',
  ipAddress: '198.51.100.20',
  signedInAt: '2026-10-20T18:30:00Z',
  lastActiveAt: '2026-11-07T21:15:00Z',
  current: false,
};

describe('SignIns', () => {
  let fixture: ComponentFixture<SignIns>;
  let page: HTMLElement;
  let server: HttpTestingController;
  let sockets: FakeSockets;
  let navigated: string[];

  beforeEach(async () => {
    sockets = new FakeSockets();
    TestBed.configureTestingModule({
      imports: [SignIns],
      providers: [
        provideRouter([]),
        provideHttpClient(),
        provideHttpClientTesting(),
        provideFakeSockets(sockets),
      ],
    });
    navigated = [];
    vi.spyOn(TestBed.inject(Router), 'navigateByUrl').mockImplementation(async (url) => {
      navigated.push(String(url));
      return true;
    });
    server = TestBed.inject(HttpTestingController);
    fixture = TestBed.createComponent(SignIns);
    page = fixture.nativeElement;
    await fixture.whenStable();
  });

  afterEach(() => server.verify());

  /** Lets the component handle a response, which takes a few turns of promises. */
  async function settle(): Promise<void> {
    for (let i = 0; i < 5; i++) {
      await new Promise((resolve) => setTimeout(resolve));
      await fixture.whenStable();
    }
  }

  /** Answers the request for the sign-ins with `signIns`. */
  async function load(...signIns: SignIn[]): Promise<void> {
    server.expectOne({ method: 'GET', url: '/api/sign-ins' }).flush({ signIns });
    await settle();
  }

  /** The text of each row, its separate pieces separated by spaces. */
  function rows(): string[] {
    return [...page.querySelectorAll('li')].map((row) => {
      const texts: string[] = [];
      const walker = document.createTreeWalker(row, NodeFilter.SHOW_TEXT);
      while (walker.nextNode()) {
        texts.push(walker.currentNode.textContent!);
      }
      return texts.join(' ').replace(/\s+/g, ' ').trim();
    });
  }

  function button(label: string): HTMLButtonElement {
    const found = [...page.querySelectorAll('button')].find(
      (button) => (button.getAttribute('aria-label') ?? button.textContent!.trim()) === label,
    );
    if (!found) {
      throw new Error(`no button "${label}"`);
    }
    return found;
  }

  function alert(): string {
    return page.querySelector('[role=alert]')?.textContent?.replace(/\s+/g, ' ').trim() ?? '';
  }

  it('lists the sign-ins, this browser first', async () => {
    expect(page.querySelector('app-skeleton')).not.toBeNull();

    await load(LAPTOP, PHONE);

    expect(page.querySelector('app-skeleton')).toBeNull();
    const [laptop, phone] = rows();
    expect(laptop).toMatch(/^Firefox on Linux This browser IP address 203\.0\.113\.7 Active now/);
    expect(phone).toMatch(
      /^Safari on iOS IP address 198\.51\.100\.20 Last active .+ · Signed in .+/,
    );
    // Every button says which sign-in it ends.
    expect(button('Sign out of this browser')).toBeTruthy();
    expect(button('Sign out of Safari on iOS')).toBeTruthy();
    expect(button('Sign out everywhere else')).toBeTruthy();
  });

  it('offers no way out of other browsers when there are none', async () => {
    await load(LAPTOP);

    expect(() => button('Sign out everywhere else')).toThrow();
  });

  it('reloads after a resync and when the sign-ins change elsewhere', async () => {
    await load(LAPTOP);
    const connection = TestBed.inject(ServerConnection);
    connection.start();

    sockets.last.receive({ type: 'hello', heartbeatTimeoutMs: 8000 });
    await load(LAPTOP, PHONE);
    expect(rows().length).toBe(2);

    sockets.last.receive({ type: 'event', name: 'device.status', data: {} });
    server.expectNone('/api/sign-ins');
    sockets.last.receive({ type: 'event', name: 'sign_ins.changed', data: {} });
    await load(LAPTOP);
    expect(rows().length).toBe(1);
    connection.stop();
  });

  it('signs out of another browser', async () => {
    await load(LAPTOP, PHONE);

    button('Sign out of Safari on iOS').click();
    await fixture.whenStable();

    const request = server.expectOne(`/api/sign-ins/${PHONE.id}`);
    expect(request.request.method).toBe('DELETE');
    expect(button('Sign out of this browser').disabled).toBe(true);
    request.flush(null, { status: 204, statusText: 'No Content' });
    await settle();
    expect(
      TestBed.inject(Toasts)
        .toasts()
        .map((toast) => toast.message),
    ).toEqual(['Signed out of Safari on iOS.']);
    await load(LAPTOP);
    expect(rows().length).toBe(1);
    expect(navigated).toEqual([]);
  });

  it('only reloads when the other browser was signed out already', async () => {
    await load(LAPTOP, PHONE);

    button('Sign out of Safari on iOS').click();
    server
      .expectOne(`/api/sign-ins/${PHONE.id}`)
      .flush({ error: 'sign_in_ended' }, { status: 410, statusText: 'Gone' });
    await settle();

    await load(LAPTOP);
    expect(alert()).toBe('');
    expect(TestBed.inject(Toasts).toasts()).toEqual([]);
  });

  it('says so when signing out of another browser fails', async () => {
    await load(LAPTOP, PHONE);

    button('Sign out of Safari on iOS').click();
    server
      .expectOne(`/api/sign-ins/${PHONE.id}`)
      .error(new ProgressEvent('error'), { status: 0, statusText: 'Unknown Error' });
    await settle();

    expect(alert()).toContain("can't be reached");
    expect(button('Sign out of Safari on iOS').disabled).toBe(false);
  });

  it('signs this browser out, and goes to the sign-in page', async () => {
    await load(LAPTOP, PHONE);

    button('Sign out of this browser').click();
    server.expectOne({ method: 'POST', url: '/api/sign-out' }).flush(null, {
      status: 204,
      statusText: 'No Content',
    });
    await settle();

    expect(navigated).toEqual(['/sign-in']);
  });

  it('signs out everywhere else once the user confirms', async () => {
    const confirm = vi.spyOn(TestBed.inject(Confirmation), 'confirm').mockResolvedValue(false);
    await load(LAPTOP, PHONE);

    button('Sign out everywhere else').click();
    await settle();
    expect(confirm).toHaveBeenCalledWith(
      expect.objectContaining({ confirmLabel: 'Sign out everywhere else' }),
    );
    server.expectNone({ method: 'DELETE', url: '/api/sign-ins' });

    confirm.mockResolvedValue(true);
    button('Sign out everywhere else').click();
    await settle();
    const request = server.expectOne({ method: 'DELETE', url: '/api/sign-ins' });
    request.flush(null, { status: 204, statusText: 'No Content' });
    await settle();

    expect(
      TestBed.inject(Toasts)
        .toasts()
        .map((toast) => toast.message),
    ).toEqual(['Signed out everywhere else.']);
    await load(LAPTOP);
    expect(navigated).toEqual([]);
  });

  it('says so when the sign-ins cannot be loaded', async () => {
    server
      .expectOne('/api/sign-ins')
      .error(new ProgressEvent('error'), { status: 0, statusText: 'Unknown Error' });
    await settle();

    expect(page.querySelector('app-skeleton')).toBeNull();
    expect(alert()).toContain("can't be reached");
  });
});
