import { HttpClient, HttpHeaders, provideHttpClient } from '@angular/common/http';
import { HttpTestingController, provideHttpClientTesting } from '@angular/common/http/testing';
import { ComponentFixture, TestBed } from '@angular/core/testing';
import { ReauthenticateDialog } from './reauthenticate-dialog';
import { CANCELLED, Reauthentication } from './reauthentication';

// jsdom has no modal dialogs.
function fakeDialogs(): void {
  Object.defineProperty(HTMLDialogElement.prototype, 'open', {
    configurable: true,
    get(this: HTMLDialogElement) {
      return this.hasAttribute('open');
    },
  });
  HTMLDialogElement.prototype.showModal = function (this: HTMLDialogElement) {
    this.setAttribute('open', '');
  };
  HTMLDialogElement.prototype.close = function (this: HTMLDialogElement) {
    if (this.hasAttribute('open')) {
      this.removeAttribute('open');
      this.dispatchEvent(new Event('close'));
    }
  };
}

describe('Reauthentication', () => {
  let fixture: ComponentFixture<ReauthenticateDialog>;
  let reauthentication: Reauthentication;
  let http: HttpClient;
  let server: HttpTestingController;
  let dialog: HTMLDialogElement;

  beforeAll(fakeDialogs);

  beforeEach(async () => {
    TestBed.configureTestingModule({
      providers: [provideHttpClient(), provideHttpClientTesting()],
    });
    fixture = TestBed.createComponent(ReauthenticateDialog);
    reauthentication = TestBed.inject(Reauthentication);
    http = TestBed.inject(HttpClient);
    server = TestBed.inject(HttpTestingController);
    dialog = fixture.nativeElement.querySelector('dialog');
    await fixture.whenStable();
  });

  afterEach(() => server.verify());

  /** Lets the dialog and the service handle a response, which takes a few turns of promises. */
  async function settle(): Promise<void> {
    for (let i = 0; i < 5; i++) {
      await new Promise((resolve) => setTimeout(resolve));
      await fixture.whenStable();
    }
  }

  function input(id: string): HTMLInputElement | null {
    return dialog.querySelector<HTMLInputElement>(`#${id}`);
  }

  async function type(id: string, value: string): Promise<void> {
    input(id)!.value = value;
    input(id)!.dispatchEvent(new Event('input'));
    await fixture.whenStable();
  }

  function button(label: string): HTMLButtonElement {
    const found = [...dialog.querySelectorAll('button')].find(
      (button) => button.textContent!.trim() === label,
    );
    if (!found) {
      throw new Error(`no button "${label}"`);
    }
    return found;
  }

  function alert(): string {
    return dialog.querySelector('[role=alert]')?.textContent?.replace(/\s+/g, ' ').trim() ?? '';
  }

  function errors(): string[] {
    return Array.from(dialog.querySelectorAll('.field-error'), (error) =>
      error.textContent!.trim(),
    );
  }

  /** Runs a change of MFA settings, which the server answers `428` until re-authenticated. */
  async function change(): Promise<{ result: Promise<unknown> }> {
    const result = reauthentication.run(() => http.delete('/api/mfa'));
    await settle();
    return { result };
  }

  function needsReauthentication(secondFactor: boolean): void {
    server
      .expectOne({ method: 'DELETE', url: '/api/mfa' })
      .flush(
        { error: 'reauthentication_required', secondFactor },
        { status: 428, statusText: 'Precondition Required' },
      );
  }

  it('sends a change without asking while the sign-in is recent', async () => {
    const { result } = await change();

    server
      .expectOne({ method: 'DELETE', url: '/api/mfa' })
      .flush(null, { status: 204, statusText: 'No Content' });

    expect(await result).toBeNull();
    expect(dialog.open).toBe(false);
  });

  it('asks for the password, then sends the change again', async () => {
    const { result } = await change();
    needsReauthentication(false);
    await settle();

    expect(dialog.open).toBe(true);
    expect(dialog.textContent).toContain('To make this change, enter your password.');
    expect(input('reauthenticate-code')).toBeNull();
    expect(input('reauthenticate-password')!.getAttribute('autocomplete')).toBe('current-password');

    await type('reauthenticate-password', 'violet otter harbor 92');
    button('Continue').click();
    await settle();
    const request = server.expectOne('/api/reauthenticate');
    expect(request.request.method).toBe('POST');
    expect(request.request.body).toEqual({ password: 'violet otter harbor 92' });
    expect(dialog.querySelector('[role=status]')!.textContent).toContain('Checking…');
    request.flush(null, { status: 204, statusText: 'No Content' });
    await settle();

    expect(dialog.open).toBe(false);
    server
      .expectOne({ method: 'DELETE', url: '/api/mfa' })
      .flush(null, { status: 204, statusText: 'No Content' });
    expect(await result).toBeNull();
  });

  it('asks for a code too with MFA on', async () => {
    const { result } = await change();
    needsReauthentication(true);
    await settle();

    expect(dialog.textContent).toContain(
      'To make this change, enter your password and a code from your authenticator app.',
    );
    expect(input('reauthenticate-code')!.getAttribute('autocomplete')).toBe('one-time-code');

    // Both are needed.
    button('Continue').click();
    await settle();
    server.expectNone('/api/reauthenticate');
    expect(errors()).toEqual([
      'Enter your password.',
      'Enter a code from your authenticator app, or a recovery code.',
    ]);

    await type('reauthenticate-password', 'violet otter harbor 92');
    await type('reauthenticate-code', ' 123456 ');
    button('Continue').click();
    await settle();
    const request = server.expectOne('/api/reauthenticate');
    expect(request.request.body).toEqual({ password: 'violet otter harbor 92', code: '123456' });
    request.flush(null, { status: 204, statusText: 'No Content' });
    await settle();

    server
      .expectOne({ method: 'DELETE', url: '/api/mfa' })
      .flush(null, { status: 204, statusText: 'No Content' });
    expect(await result).toBeNull();
  });

  it('says what was wrong, and lets the user try again', async () => {
    const { result } = await change();
    needsReauthentication(true);
    await settle();

    await type('reauthenticate-password', 'wrong password');
    await type('reauthenticate-code', '000000');
    button('Continue').click();
    await settle();
    server
      .expectOne('/api/reauthenticate')
      .flush({ error: 'reauthentication_failed' }, { status: 401, statusText: 'Unauthorized' });
    await settle();

    expect(dialog.open).toBe(true);
    expect(alert()).toContain("The password or code isn't right.");
    expect(input('reauthenticate-password')!.value).toBe('');
    expect(input('reauthenticate-code')!.value).toBe('');
    expect(document.activeElement).toBe(input('reauthenticate-password'));

    await type('reauthenticate-password', 'violet otter harbor 92');
    await type('reauthenticate-code', '123456');
    button('Continue').click();
    await settle();
    server.expectOne('/api/reauthenticate').flush(
      { error: 'too_many_requests' },
      {
        status: 429,
        statusText: 'Too Many Requests',
        headers: new HttpHeaders({ 'Retry-After': '900' }),
      },
    );
    await settle();
    expect(alert()).toContain('There have been too many wrong attempts.');

    button('Cancel').click();
    await settle();
    expect(await result).toBe(CANCELLED);
  });

  it('resolves to CANCELLED when the user cancels, with Escape too', async () => {
    const { result } = await change();
    needsReauthentication(false);
    await settle();

    // Escape closes the dialog.
    dialog.close();
    await settle();

    expect(await result).toBe(CANCELLED);
    expect(reauthentication.request()).toBeUndefined();
  });

  it('starts with empty fields every time', async () => {
    const first = await change();
    needsReauthentication(false);
    await settle();
    await type('reauthenticate-password', 'typed before');
    button('Cancel').click();
    await settle();
    await first.result;

    await change();
    needsReauthentication(false);
    await settle();

    expect(input('reauthenticate-password')!.value).toBe('');
    reauthentication.answer(false);
  });

  it('passes other errors on', async () => {
    const { result } = await change();
    server
      .expectOne({ method: 'DELETE', url: '/api/mfa' })
      .flush({ error: 'internal' }, { status: 500, statusText: 'Internal Server Error' });

    await expect(result).rejects.toMatchObject({ status: 500 });
    expect(dialog.open).toBe(false);
  });
});
