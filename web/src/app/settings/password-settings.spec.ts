import { HttpHeaders, provideHttpClient } from '@angular/common/http';
import { HttpTestingController, provideHttpClientTesting } from '@angular/common/http/testing';
import { ComponentFixture, TestBed } from '@angular/core/testing';
import { provideRouter } from '@angular/router';
import { provideFakeSockets } from '../shared/connection/testing';
import { CANCELLED, Reauthentication } from '../shared/reauthentication/reauthentication';
import { Toasts } from '../shared/toast/toasts';
import { PasswordSettings } from './password-settings';

const CURRENT = 'violet otter harbor 92';
const NEW = 'amber lantern meadow 41';

describe('PasswordSettings', () => {
  let fixture: ComponentFixture<PasswordSettings>;
  let page: HTMLElement;
  let server: HttpTestingController;

  beforeEach(async () => {
    TestBed.configureTestingModule({
      imports: [PasswordSettings],
      providers: [
        provideRouter([]),
        provideHttpClient(),
        provideHttpClientTesting(),
        provideFakeSockets(),
      ],
    });
    server = TestBed.inject(HttpTestingController);
    fixture = TestBed.createComponent(PasswordSettings);
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

  function input(id: string): HTMLInputElement {
    return page.querySelector<HTMLInputElement>(`#${id}`)!;
  }

  async function type(id: string, value: string): Promise<void> {
    input(id).value = value;
    input(id).dispatchEvent(new Event('input'));
    await fixture.whenStable();
  }

  async function fill(current: string, next: string): Promise<void> {
    await type('settings-current-password', current);
    await type('settings-new-password', next);
  }

  async function submit(): Promise<void> {
    page.querySelector<HTMLButtonElement>('button[type=submit]')!.click();
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

  it('asks password managers for the current password and a new one', () => {
    expect(input('settings-current-password').autocomplete).toBe('current-password');
    expect(input('settings-new-password').autocomplete).toBe('new-password');
    expect(input('settings-new-password').type).toBe('password');
  });

  it('changes the password', async () => {
    await fill(CURRENT, NEW);

    await submit();

    const request = server.expectOne({ method: 'POST', url: '/api/account/password' });
    expect(request.request.body).toEqual({ currentPassword: CURRENT, newPassword: NEW });
    request.flush(null, { status: 204, statusText: 'No Content' });
    await settle();
    expect(toasts()).toEqual(['Password changed. Your other browsers are signed out.']);
    expect(input('settings-current-password').value).toBe('');
    expect(input('settings-new-password').value).toBe('');
    expect(fieldError('settings-current-password')).toBeNull();
  });

  it('checks the fields before sending them', async () => {
    await fill('', 'short');

    await submit();

    server.expectNone({ method: 'POST', url: '/api/account/password' });
    expect(fieldError('settings-current-password')).toBe('Enter your current password.');
    expect(fieldError('settings-new-password')).toBe('Use at least 12 characters.');
    expect(document.activeElement).toBe(input('settings-current-password'));
  });

  it("shows the server's answers for the fields", async () => {
    await fill('wrong password 1', 'password1234');

    await submit();
    server.expectOne({ method: 'POST', url: '/api/account/password' }).flush(
      {
        error: 'invalid',
        fields: {
          currentPassword: { code: 'incorrect' },
          newPassword: { code: 'compromised' },
        },
      },
      { status: 422, statusText: 'Unprocessable Entity' },
    );
    await settle();

    expect(fieldError('settings-current-password')).toBe("That isn't your current password.");
    expect(fieldError('settings-new-password')).toBe(
      'This password is on a list of passwords stolen from other sites. Choose another one.',
    );
    expect(document.activeElement).toBe(input('settings-current-password'));
    await type('settings-current-password', CURRENT);
    expect(fieldError('settings-current-password')).toBeNull();
  });

  it('says when to try again while passwords are blocked', async () => {
    await fill(CURRENT, NEW);

    await submit();
    server.expectOne({ method: 'POST', url: '/api/account/password' }).flush(
      { error: 'too_many_requests' },
      {
        status: 429,
        statusText: 'Too Many Requests',
        headers: new HttpHeaders({ 'Retry-After': '900' }),
      },
    );
    await settle();

    expect(alert()).toMatch(
      /Too many wrong passwords have been entered for your account\. ?Try again after .+\./,
    );
  });

  it('keeps the form when the user cancels re-authenticating', async () => {
    vi.spyOn(TestBed.inject(Reauthentication), 'run').mockResolvedValue(CANCELLED);
    await fill(CURRENT, NEW);

    await submit();

    expect(toasts()).toEqual([]);
    expect(input('settings-new-password').value).toBe(NEW);
  });
});
