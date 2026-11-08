import { Location } from '@angular/common';
import { provideHttpClient } from '@angular/common/http';
import { HttpTestingController, provideHttpClientTesting } from '@angular/common/http/testing';
import { provideLocationMocks } from '@angular/common/testing';
import { TestBed } from '@angular/core/testing';
import { provideRouter } from '@angular/router';
import { RouterTestingHarness } from '@angular/router/testing';
import { ConfirmEmail } from './confirm-email';

describe('ConfirmEmail', () => {
  let server: HttpTestingController;
  let harness: RouterTestingHarness;
  let page: HTMLElement;

  beforeEach(() => {
    TestBed.configureTestingModule({
      providers: [
        provideRouter([{ path: 'confirm-email', component: ConfirmEmail }]),
        provideLocationMocks(),
        provideHttpClient(),
        provideHttpClientTesting(),
      ],
    });
    server = TestBed.inject(HttpTestingController);
  });

  afterEach(() => server.verify());

  async function open(url: string): Promise<void> {
    harness = await RouterTestingHarness.create(url);
    page = harness.routeNativeElement!;
  }

  /** Lets the component handle a response, which takes a few turns of promises. */
  async function settle(): Promise<void> {
    for (let i = 0; i < 5; i++) {
      await new Promise((resolve) => setTimeout(resolve));
      await harness.fixture.whenStable();
    }
  }

  function text(): string {
    return page.textContent!.replace(/\s+/g, ' ').trim();
  }

  function answer(body: object, status: number, statusText: string): void {
    const request = server.expectOne({ method: 'POST', url: '/api/account/email/confirm' });
    expect(request.request.body).toEqual({ token: '0123abcd' });
    request.flush(body, { status, statusText });
  }

  it('changes the address with the token, and takes it out of the address bar', async () => {
    await open('/confirm-email#0123abcd');

    expect(TestBed.inject(Location).path(true)).toBe('/confirm-email');
    expect(text()).toContain('Changing your email address…');
    answer({ email: 'ann.new@example.com' }, 200, 'OK');
    await settle();

    expect(text()).toContain(
      'Your email address is now ann.new@example.com. Sign in with it from now on.',
    );
    expect(page.querySelector('a[href="/settings"]')).not.toBeNull();
  });

  it('says when the link has expired', async () => {
    await open('/confirm-email#0123abcd');

    answer({ error: 'link_expired' }, 410, 'Gone');
    await settle();

    expect(text()).toContain("This link has expired or isn't valid");
    expect(text()).toContain('A link works once, for 24 hours');
  });

  it('says when the address has got an account meanwhile', async () => {
    await open('/confirm-email#0123abcd');

    answer({ error: 'email_in_use' }, 409, 'Conflict');
    await settle();

    expect(text()).toContain('This address has got an account of its own');
  });

  it('explains the page without a token', async () => {
    await open('/confirm-email');
    await settle();

    server.expectNone({ method: 'POST', url: '/api/account/email/confirm' });
    expect(text()).toContain('open the link in the email we sent to the new address');
  });

  it('tries again after a failure', async () => {
    await open('/confirm-email#0123abcd');
    answer({ error: 'internal' }, 500, 'Internal Server Error');
    await settle();
    expect(page.querySelector('[role=alert]')).not.toBeNull();

    [...page.querySelectorAll('button')]
      .find((b) => b.textContent!.trim() === 'Try again')!
      .click();
    await settle();
    answer({ email: 'ann.new@example.com' }, 200, 'OK');
    await settle();

    expect(text()).toContain('Your email address is now ann.new@example.com.');
  });
});
