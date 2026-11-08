import { HttpHeaders, provideHttpClient } from '@angular/common/http';
import { HttpTestingController, provideHttpClientTesting } from '@angular/common/http/testing';
import { ComponentFixture, TestBed } from '@angular/core/testing';
import { provideRouter } from '@angular/router';
import { ServerConnection } from '../shared/connection/server-connection';
import { FakeSockets, provideFakeSockets } from '../shared/connection/testing';
import { CANCELLED, Reauthentication } from '../shared/reauthentication/reauthentication';
import { Toasts } from '../shared/toast/toasts';
import { Account, AccountSettings } from './account-settings';

const ANN: Account = {
  id: '0b6f8e0c-5d1a-4c5e-9a43-3f2a1c9d7e10',
  email: 'ann@example.com',
  displayName: 'Ann Lee',
  emailVerified: true,
};

describe('AccountSettings', () => {
  let fixture: ComponentFixture<AccountSettings>;
  let page: HTMLElement;
  let server: HttpTestingController;
  let sockets: FakeSockets;

  beforeEach(async () => {
    sockets = new FakeSockets();
    TestBed.configureTestingModule({
      imports: [AccountSettings],
      providers: [
        provideRouter([]),
        provideHttpClient(),
        provideHttpClientTesting(),
        provideFakeSockets(sockets),
      ],
    });
    server = TestBed.inject(HttpTestingController);
    fixture = TestBed.createComponent(AccountSettings);
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

  async function load(account: Account): Promise<void> {
    server.expectOne({ method: 'GET', url: '/api/account' }).flush(account);
    await settle();
  }

  function text(): string {
    return page.textContent!.replace(/\s+/g, ' ').trim();
  }

  function input(id: string): HTMLInputElement {
    return page.querySelector<HTMLInputElement>(`#${id}`)!;
  }

  async function type(id: string, value: string): Promise<void> {
    input(id).value = value;
    input(id).dispatchEvent(new Event('input'));
    await fixture.whenStable();
  }

  async function click(label: string): Promise<void> {
    const button = [...page.querySelectorAll('button')].find(
      (button) => button.textContent!.trim() === label,
    );
    if (!button) {
      throw new Error(`no button "${label}"`);
    }
    button.click();
    await settle();
  }

  function fieldError(id: string): string | null {
    return page.querySelector(`#${id}-error`)?.textContent?.trim() ?? null;
  }

  function alert(): string {
    return page.querySelector('[role=alert]')?.textContent?.replace(/\s+/g, ' ').trim() ?? '';
  }

  function toasts(): string[] {
    return TestBed.inject(Toasts)
      .toasts()
      .map((toast) => toast.message);
  }

  it('shows the name and the email address', async () => {
    expect(page.querySelectorAll('app-skeleton').length).toBe(2);

    await load(ANN);

    expect(page.querySelector('app-skeleton')).toBeNull();
    expect(input('settings-display-name').value).toBe('Ann Lee');
    expect(text()).toContain('ann@example.com');
    expect(text()).not.toContain('Not verified');
    expect(text()).not.toContain('Waiting for');
  });

  it('says when the address is not verified', async () => {
    await load({ ...ANN, emailVerified: false });

    expect(text()).toContain('Not verified');
  });

  it('saves the name', async () => {
    await load(ANN);
    await type('settings-display-name', 'Ann B. Lee');

    await click('Save name');

    const request = server.expectOne({ method: 'PATCH', url: '/api/account' });
    expect(request.request.body).toEqual({ displayName: 'Ann B. Lee' });
    request.flush(null, { status: 204, statusText: 'No Content' });
    await settle();
    expect(toasts()).toEqual(['Name saved.']);
    await load({ ...ANN, displayName: 'Ann B. Lee' });
    expect(input('settings-display-name').value).toBe('Ann B. Lee');
  });

  it('checks the name before sending it', async () => {
    await load(ANN);
    await type('settings-display-name', '   ');

    await click('Save name');

    server.expectNone({ method: 'PATCH', url: '/api/account' });
    expect(fieldError('settings-display-name')).toBe('Enter your name.');
    expect(document.activeElement).toBe(input('settings-display-name'));
  });

  it("shows the server's answer for the name", async () => {
    await load(ANN);
    await type('settings-display-name', 'Ann\u0007Lee');

    await click('Save name');
    server
      .expectOne({ method: 'PATCH', url: '/api/account' })
      .flush(
        { error: 'invalid', fields: { displayName: { code: 'invalid' } } },
        { status: 422, statusText: 'Unprocessable Entity' },
      );
    await settle();

    expect(fieldError('settings-display-name')).toBe(
      'Use letters, numbers, spaces and punctuation only.',
    );
    await type('settings-display-name', 'Ann Lee');
    expect(fieldError('settings-display-name')).toBeNull();
  });

  /** Connects to the server, as the app's layout does, and returns the connection. */
  function connect(): ServerConnection {
    const connection = TestBed.inject(ServerConnection);
    connection.start();
    sockets.last.receive({ type: 'hello', heartbeatTimeoutMs: 8000 });
    return connection;
  }

  it('keeps a name being typed when the account reloads', async () => {
    await load(ANN);
    const connection = connect();
    await load(ANN);
    await type('settings-display-name', 'Ann B');

    sockets.last.receive({ type: 'event', name: 'account.changed', data: {} });
    await load({ ...ANN, email: 'ann.new@example.com' });

    expect(input('settings-display-name').value).toBe('Ann B');
    expect(text()).toContain('ann.new@example.com');
    connection.stop();
  });

  it('emails a link to a new address, and shows the change waiting', async () => {
    await load(ANN);
    await type('settings-new-email', ' ann.new@example.com ');

    await click('Send link');

    const request = server.expectOne({ method: 'POST', url: '/api/account/email' });
    expect(request.request.body).toEqual({ email: 'ann.new@example.com' });
    request.flush(null, { status: 202, statusText: 'Accepted' });
    await settle();
    expect(toasts()).toEqual(['We sent an email to ann.new@example.com.']);
    expect(input('settings-new-email').value).toBe('');
    await load({ ...ANN, pendingEmail: 'ann.new@example.com' });
    expect(text()).toContain(
      'Waiting for ann.new@example.com: open the link in the email we sent there to make it your address. The link works for 24 hours.',
    );
    expect(text()).toContain('ann@example.com');
  });

  it('checks the new address before sending it', async () => {
    await load(ANN);
    await type('settings-new-email', 'not an address');

    await click('Send link');

    server.expectNone({ method: 'POST', url: '/api/account/email' });
    expect(fieldError('settings-new-email')).toBe(
      'Enter an email address such as name@example.com.',
    );
  });

  it('says when the new address is the current one', async () => {
    await load(ANN);
    await type('settings-new-email', 'ANN@example.com');

    await click('Send link');
    server
      .expectOne({ method: 'POST', url: '/api/account/email' })
      .flush(
        { error: 'invalid', fields: { email: { code: 'unchanged' } } },
        { status: 422, statusText: 'Unprocessable Entity' },
      );
    await settle();

    expect(fieldError('settings-new-email')).toBe(
      'This is your email address already. Enter another one.',
    );
    expect(document.activeElement).toBe(input('settings-new-email'));
  });

  it('says when to try again after too many requests', async () => {
    await load(ANN);
    await type('settings-new-email', 'ann.new@example.com');

    await click('Send link');
    server.expectOne({ method: 'POST', url: '/api/account/email' }).flush(
      { error: 'too_many_requests' },
      {
        status: 429,
        statusText: 'Too Many Requests',
        headers: new HttpHeaders({ 'Retry-After': '3600' }),
      },
    );
    await settle();

    expect(alert()).toMatch(
      /You have asked to change your email address too often\. ?Try again after .+\./,
    );
  });

  it('does nothing when the user cancels re-authenticating', async () => {
    vi.spyOn(TestBed.inject(Reauthentication), 'run').mockResolvedValue(CANCELLED);
    await load(ANN);
    await type('settings-new-email', 'ann.new@example.com');

    await click('Send link');

    expect(toasts()).toEqual([]);
    expect(alert()).toBe('');
    expect(input('settings-new-email').value).toBe('ann.new@example.com');
  });

  it('reloads after a resync and when the account changes elsewhere', async () => {
    await load(ANN);
    const connection = connect();
    await load(ANN);

    sockets.last.receive({ type: 'event', name: 'mfa.changed', data: {} });
    server.expectNone('/api/account');
    sockets.last.receive({ type: 'event', name: 'account.changed', data: {} });
    await load({ ...ANN, email: 'ann.new@example.com', displayName: 'Ann B. Lee' });

    expect(text()).toContain('ann.new@example.com');
    expect(input('settings-display-name').value).toBe('Ann B. Lee');
    connection.stop();
  });
});
